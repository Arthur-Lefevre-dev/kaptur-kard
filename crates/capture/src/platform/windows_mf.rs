//! Windows Media Foundation video capture behind [`VideoSource`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use tracing::{debug, info, warn};
use windows::core::{GUID, PWSTR};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFMediaBuffer, IMFMediaSource, IMFMediaType, IMFSample,
    IMFSourceReader, MFCreateAttributes, MFCreateDeviceSource, MFCreateMediaType,
    MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFMediaType_Video, MFStartup,
    MFVideoFormat_ARGB32, MFVideoFormat_MJPG, MFVideoFormat_NV12, MFVideoFormat_RGB32,
    MFVideoFormat_YUY2, MFSTARTUP_FULL, MF_API_VERSION, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

use crate::device::{DeviceId, DeviceInfo, FormatDesc};
use crate::format::{looks_interlaced, PixelFormat, Rational};
use crate::labels::{device_capture_score_for, device_sort_key, is_virtual_camera};
use crate::pool::BufferPool;
use crate::source::{
    frame_channel, CaptureError, FrameReceiver, FrameSender, VideoFrame, VideoSource,
};

static MF_START: Once = Once::new();
static MF_START_OK: AtomicBool = AtomicBool::new(false);

fn pack_u32x2(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | (lo as u64)
}

fn unpack_u32x2(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

fn ensure_mf() -> Result<(), CaptureError> {
    // Do NOT CoInitialize on the caller thread — winit needs STA/OleInitialize on the UI thread.
    let mut start_err: Option<String> = None;
    MF_START.call_once(|| unsafe {
        match MFStartup(MF_VERSION, MFSTARTUP_FULL) {
            Ok(()) => {
                MF_START_OK.store(true, Ordering::SeqCst);
                info!(version = MF_API_VERSION, "Media Foundation started");
            }
            Err(e) => {
                start_err = Some(format!("MFStartup failed: {e}"));
            }
        }
    });
    if MF_START_OK.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(CaptureError::DeviceOpen(
            start_err.unwrap_or_else(|| "Media Foundation unavailable".into()),
        ))
    }
}

fn co_init_mta() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// Run Media Foundation work on a short-lived MTA thread (safe vs winit UI STA).
fn run_on_mta<T, F>(f: F) -> Result<T, CaptureError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, CaptureError> + Send + 'static,
{
    let (tx, rx) = crossbeam_channel::bounded(1);
    thread::Builder::new()
        .name("mf-mta".into())
        .spawn(move || {
            co_init_mta();
            let _ = tx.send(f());
        })
        .map_err(|e| CaptureError::Other(Box::new(e)))?;
    rx.recv()
        .map_err(|_| CaptureError::DeviceOpen("MF worker disconnected".into()))?
}

fn guid_to_pixel_format(guid: GUID) -> PixelFormat {
    if guid == MFVideoFormat_NV12 {
        PixelFormat::Nv12
    } else if guid == MFVideoFormat_YUY2 {
        PixelFormat::Yuy2
    } else if guid == MFVideoFormat_RGB32 || guid == MFVideoFormat_ARGB32 {
        PixelFormat::Bgra8
    } else if guid == MFVideoFormat_MJPG {
        PixelFormat::Mjpeg
    } else {
        // Pack first 4 bytes of GUID Data1 as fourcc-ish diagnostic.
        PixelFormat::Unknown(guid.data1)
    }
}

fn pixel_format_to_guid(fmt: PixelFormat) -> Option<GUID> {
    match fmt {
        PixelFormat::Nv12 => Some(MFVideoFormat_NV12),
        PixelFormat::Yuy2 => Some(MFVideoFormat_YUY2),
        PixelFormat::Bgra8 => Some(MFVideoFormat_RGB32),
        PixelFormat::Mjpeg => Some(MFVideoFormat_MJPG),
        PixelFormat::Rgb24 | PixelFormat::Unknown(_) => None,
    }
}

fn read_allocated_string(attrs: &IMFAttributes, key: &GUID) -> Result<String, CaptureError> {
    unsafe {
        let mut pwstr = PWSTR::null();
        let mut len = 0u32;
        attrs
            .GetAllocatedString(key, &mut pwstr, &mut len)
            .map_err(|e| CaptureError::DeviceOpen(format!("GetAllocatedString: {e}")))?;
        if pwstr.is_null() {
            return Ok(String::new());
        }
        let s = pwstr.to_string().unwrap_or_default();
        CoTaskMemFree(Some(pwstr.as_ptr() as *const _));
        Ok(s)
    }
}

fn frame_size_of(mt: &IMFMediaType) -> Result<(u32, u32), CaptureError> {
    unsafe {
        let v = mt
            .GetUINT64(&MF_MT_FRAME_SIZE)
            .map_err(|e| CaptureError::Negotiate(format!("FRAME_SIZE: {e}")))?;
        Ok(unpack_u32x2(v))
    }
}

fn frame_rate_of(mt: &IMFMediaType) -> Result<Rational, CaptureError> {
    unsafe {
        let v = mt
            .GetUINT64(&MF_MT_FRAME_RATE)
            .map_err(|e| CaptureError::Negotiate(format!("FRAME_RATE: {e}")))?;
        let (num, den) = unpack_u32x2(v);
        Ok(Rational::new(num.max(1), den.max(1)))
    }
}

fn subtype_of(mt: &IMFMediaType) -> Result<GUID, CaptureError> {
    unsafe {
        mt.GetGUID(&MF_MT_SUBTYPE)
            .map_err(|e| CaptureError::Negotiate(format!("SUBTYPE: {e}")))
    }
}

/// MFVideoInterlace_Progressive = 2; anything else non-zero is treated as interlaced.
fn interlace_flag_of(mt: &IMFMediaType) -> bool {
    unsafe {
        match mt.GetUINT32(&MF_MT_INTERLACE_MODE) {
            Ok(mode) => mode != 0 && mode != 2,
            Err(_) => false,
        }
    }
}

fn merge_format(formats: &mut Vec<FormatDesc>, desc: FormatDesc) {
    // Keep progressive and interlaced variants separate (same size can advertise both).
    if let Some(existing) = formats.iter_mut().find(|f| {
        f.pixel_format == desc.pixel_format
            && f.width == desc.width
            && f.height == desc.height
            && f.interlaced == desc.interlaced
    }) {
        for r in desc.frame_rates {
            if !existing
                .frame_rates
                .iter()
                .any(|x| (x.as_f64() - r.as_f64()).abs() < 0.05)
            {
                existing.frame_rates.push(r);
            }
        }
    } else {
        formats.push(desc);
    }
}

fn enumerate_native_types(reader: &IMFSourceReader) -> Result<Vec<FormatDesc>, CaptureError> {
    let mut formats = Vec::new();
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    for index in 0..512u32 {
        let mt = unsafe { reader.GetNativeMediaType(stream, index) };
        let mt = match mt {
            Ok(mt) => mt,
            Err(_) => break,
        };
        let subtype = match subtype_of(&mt) {
            Ok(g) => g,
            Err(_) => continue,
        };
        let (width, height) = match frame_size_of(&mt) {
            Ok(wh) => wh,
            Err(_) => continue,
        };
        let rate = match frame_rate_of(&mt) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if width == 0 || height == 0 {
            continue;
        }
        let pixel_format = guid_to_pixel_format(subtype);
        if matches!(pixel_format, PixelFormat::Unknown(_)) {
            continue;
        }
        // Some capture drivers mark progressive 1080p50/60 as interlaced.
        // Prefer the rate/size heuristic when the mode clearly looks progressive.
        let flag = interlace_flag_of(&mt);
        let heuristic = looks_interlaced(width, height, rate);
        let interlaced = if flag && !heuristic && rate.as_f64() >= 48.0 && height >= 720 {
            false
        } else {
            flag || heuristic
        };
        merge_format(
            &mut formats,
            FormatDesc {
                pixel_format,
                width,
                height,
                frame_rates: vec![rate],
                interlaced,
            },
        );
    }
    Ok(formats)
}

fn create_attributes(size: u32) -> Result<IMFAttributes, CaptureError> {
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, size)
            .map_err(|e| CaptureError::DeviceOpen(format!("MFCreateAttributes: {e}")))?;
        attrs.ok_or_else(|| CaptureError::DeviceOpen("MFCreateAttributes returned null".into()))
    }
}

fn open_reader_from_symlink(symlink: &str) -> Result<(IMFMediaSource, IMFSourceReader), CaptureError> {
    ensure_mf()?;
    unsafe {
        let attrs = create_attributes(2)?;
        attrs
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(|e| CaptureError::DeviceOpen(format!("SetGUID source type: {e}")))?;

        let wide: Vec<u16> = symlink.encode_utf16().chain(std::iter::once(0)).collect();
        attrs
            .SetString(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                windows::core::PCWSTR(wide.as_ptr()),
            )
            .map_err(|e| CaptureError::DeviceOpen(format!("SetString symlink: {e}")))?;

        let source: IMFMediaSource = MFCreateDeviceSource(&attrs)
            .map_err(|e| CaptureError::DeviceOpen(format!("MFCreateDeviceSource: {e}")))?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, None::<&IMFAttributes>)
            .map_err(|e| CaptureError::DeviceOpen(format!("SourceReader: {e}")))?;
        reader
            .SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)
            .map_err(|e| CaptureError::DeviceOpen(format!("SetStreamSelection: {e}")))?;
        Ok((source, reader))
    }
}

/// Enumerate UVC / capture devices and discovered modes.
pub fn enumerate_devices() -> Result<Vec<DeviceInfo>, CaptureError> {
    run_on_mta(|| {
        ensure_mf()?;
        let mut devices = Vec::new();

        unsafe {
            let attrs = create_attributes(1)?;
            attrs
                .SetGUID(
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                )
                .map_err(|e| CaptureError::DeviceOpen(format!("SetGUID: {e}")))?;

            let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFEnumDeviceSources(&attrs, &mut activates, &mut count)
                .map_err(|e| CaptureError::DeviceOpen(format!("MFEnumDeviceSources: {e}")))?;

            if activates.is_null() || count == 0 {
                return Ok(devices);
            }

            let slice = std::slice::from_raw_parts(activates, count as usize);
            info!(count, "MFEnumDeviceSources returned video capture activates");
            for activate_opt in slice {
                let Some(activate) = activate_opt else {
                    continue;
                };
                let name = read_allocated_string(activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)
                    .unwrap_or_else(|_| "Unknown device".into());
                let symlink = read_allocated_string(
                    activate,
                    &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                )
                .unwrap_or_default();
                // Never drop a named activate solely for a missing symlink — keep it listed.
                let id = if symlink.is_empty() {
                    format!("mf:name:{}", name.replace(['\\', '/', ':'], "_"))
                } else {
                    format!("mf:{symlink}")
                };

                let (formats, enum_note) = if symlink.is_empty() {
                    warn!(%name, "MF device has empty symbolic link; listing without modes");
                    (Vec::new(), Some("no symbolic link".into()))
                } else {
                    match open_reader_from_symlink(&symlink) {
                        Ok((source, reader)) => {
                            let formats = enumerate_native_types(&reader).unwrap_or_default();
                            let _ = source.Shutdown();
                            let note = if formats.is_empty() {
                                Some("no usable modes probed".into())
                            } else {
                                None
                            };
                            (formats, note)
                        }
                        Err(e) => {
                            // Keep the device visible; start will surface a clear error.
                            warn!(
                                %name,
                                %id,
                                error = %e,
                                "MF open/probe failed — still listing device"
                            );
                            (Vec::new(), Some(format!("probe failed: {e}")))
                        }
                    }
                };

                let score = device_capture_score_for(&name, &id);
                let virtual_cam = is_virtual_camera(&name, &id);
                info!(
                    %name,
                    %id,
                    formats = formats.len(),
                    score,
                    virtual_cam,
                    note = enum_note.as_deref().unwrap_or(""),
                    "MF video capture device"
                );

                devices.push(DeviceInfo {
                    id: DeviceId::new(id),
                    display_name: name,
                    backend: "mf",
                    formats,
                    enum_note,
                });
            }

            CoTaskMemFree(Some(activates as *const _));
        }

        // Capture cards first, virtual cameras last (stable by name within a tier).
        devices.sort_by(|a, b| {
            let ka = device_sort_key(a);
            let kb = device_sort_key(b);
            kb.0.cmp(&ka.0)
                .then_with(|| kb.1.cmp(&ka.1))
                .then_with(|| ka.2.cmp(&kb.2))
        });

        Ok(devices)
    })
}

fn configure_media_type(reader: &IMFSourceReader, selected: &FormatDesc) -> Result<PixelFormat, CaptureError> {
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let target_rate = selected
        .frame_rates
        .first()
        .copied()
        .unwrap_or(Rational::new(60, 1));
    let target_guid = pixel_format_to_guid(selected.pixel_format).ok_or_else(|| {
        CaptureError::UnsupportedFormat(format!("{:?}", selected.pixel_format))
    })?;

    // Find matching native type index.
    let mut matched_index = None;
    for index in 0..512u32 {
        let mt = unsafe { reader.GetNativeMediaType(stream, index) };
        let mt = match mt {
            Ok(mt) => mt,
            Err(_) => break,
        };
        let Ok(subtype) = subtype_of(&mt) else { continue };
        let Ok((w, h)) = frame_size_of(&mt) else { continue };
        let Ok(rate) = frame_rate_of(&mt) else { continue };
        if w == selected.width
            && h == selected.height
            && subtype == target_guid
            && (rate.as_f64() - target_rate.as_f64()).abs() < 0.05
        {
            matched_index = Some(index);
            break;
        }
    }

    let index = matched_index.ok_or_else(|| {
        CaptureError::Negotiate(format!(
            "no native type for {}x{} {:?} @ {:.2} fps",
            selected.width,
            selected.height,
            selected.pixel_format,
            target_rate.as_f64()
        ))
    })?;

    unsafe {
        let native = reader
            .GetNativeMediaType(stream, index)
            .map_err(|e| CaptureError::Negotiate(format!("GetNativeMediaType: {e}")))?;
        reader
            .SetCurrentMediaType(stream, None, &native)
            .map_err(|e| CaptureError::Negotiate(format!("SetCurrentMediaType native: {e}")))?;

        // For MJPEG (and as a safety net), request NV12 output so MF inserts a decoder / converter.
        let delivered = if selected.pixel_format == PixelFormat::Mjpeg {
            let out = MFCreateMediaType()
                .map_err(|e| CaptureError::Negotiate(format!("MFCreateMediaType: {e}")))?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| CaptureError::Negotiate(format!("MAJOR: {e}")))?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(|e| CaptureError::Negotiate(format!("SUBTYPE NV12: {e}")))?;
            out.SetUINT64(
                &MF_MT_FRAME_SIZE,
                pack_u32x2(selected.width, selected.height),
            )
            .map_err(|e| CaptureError::Negotiate(format!("FRAME_SIZE: {e}")))?;
            out.SetUINT64(
                &MF_MT_FRAME_RATE,
                pack_u32x2(target_rate.num, target_rate.den),
            )
            .map_err(|e| CaptureError::Negotiate(format!("FRAME_RATE: {e}")))?;
            match reader.SetCurrentMediaType(stream, None, &out) {
                Ok(()) => PixelFormat::Nv12,
                Err(e) => {
                    warn!(error = %e, "NV12 output for MJPEG failed; trying RGB32");
                    out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                        .map_err(|e| CaptureError::Negotiate(format!("SUBTYPE RGB32: {e}")))?;
                    reader
                        .SetCurrentMediaType(stream, None, &out)
                        .map_err(|e| CaptureError::Negotiate(format!("Set RGB32 output: {e}")))?;
                    PixelFormat::Bgra8
                }
            }
        } else {
            selected.pixel_format
        };

        Ok(delivered)
    }
}

fn plane_size(fmt: PixelFormat, width: u32, height: u32) -> usize {
    let w = width as usize;
    let h = height as usize;
    match fmt {
        PixelFormat::Bgra8 | PixelFormat::Rgb24 => w * h * 4,
        PixelFormat::Yuy2 => w * h * 2,
        PixelFormat::Nv12 => w * h + (w * h / 2),
        PixelFormat::Mjpeg | PixelFormat::Unknown(_) => w * h * 4,
    }
}

fn copy_sample_to_pool(
    sample: &IMFSample,
    pool: &Arc<BufferPool>,
    expected: usize,
) -> Result<Arc<crate::pool::PooledBytes>, CaptureError> {
    unsafe {
        let buffer: IMFMediaBuffer = sample
            .ConvertToContiguousBuffer()
            .map_err(|e| CaptureError::Other(Box::new(e)))?;
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut max_len = 0u32;
        let mut cur_len = 0u32;
        buffer
            .Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))
            .map_err(|e| CaptureError::Other(Box::new(e)))?;
        let len = cur_len as usize;
        let src = if ptr.is_null() || len == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(ptr, len)
        };
        let copy_len = if expected > 0 { expected.min(len) } else { len };
        let mut pooled = pool.take(if expected > 0 { expected } else { copy_len });
        if copy_len > 0 {
            pooled.as_mut_slice()[..copy_len].copy_from_slice(&src[..copy_len]);
        }
        let _ = buffer.Unlock();
        Ok(pooled.into_arc())
    }
}

/// Media Foundation video source for one enumerated device.
pub struct MfVideoSource {
    info: DeviceInfo,
    symlink: String,
    sender: FrameSender,
    receiver: FrameReceiver,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    pool: Arc<BufferPool>,
}

impl MfVideoSource {
    pub fn open(info: DeviceInfo) -> Result<Self, CaptureError> {
        let symlink = info
            .id
            .as_str()
            .strip_prefix("mf:")
            .unwrap_or(info.id.as_str())
            .to_string();
        if symlink.is_empty() {
            return Err(CaptureError::DeviceOpen("empty symbolic link".into()));
        }
        ensure_mf()?;
        let (sender, receiver) = frame_channel();
        Ok(Self {
            info,
            symlink,
            sender,
            receiver,
            running: Arc::new(AtomicBool::new(false)),
            worker: None,
            pool: BufferPool::new(),
        })
    }
}

impl VideoSource for MfVideoSource {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn start(&mut self, selected: &FormatDesc) -> Result<(), CaptureError> {
        if self.running.load(Ordering::SeqCst) {
            return Err(CaptureError::AlreadyRunning);
        }

        // COM/MF interfaces are apartment-affine; open the reader on the capture thread.
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<PixelFormat, String>>(1);
        let symlink = self.symlink.clone();
        let selected = selected.clone();
        let width = selected.width;
        let height = selected.height;

        self.running.store(true, Ordering::SeqCst);
        let running = Arc::clone(&self.running);
        let sender = self.sender.clone();
        let pool = Arc::clone(&self.pool);
        let device_name = self.info.display_name.clone();

        self.worker = Some(
            thread::Builder::new()
                .name("mf-capture".into())
                .spawn(move || {
                    co_init_mta();
                    let opened = (|| -> Result<(IMFMediaSource, IMFSourceReader, PixelFormat), CaptureError> {
                        let (source, reader) = open_reader_from_symlink(&symlink)?;
                        let delivered = configure_media_type(&reader, &selected)?;
                        Ok((source, reader, delivered))
                    })();

                    let (source, reader, delivered) = match opened {
                        Ok(v) => {
                            let _ = ready_tx.send(Ok(v.2));
                            v
                        }
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.to_string()));
                            running.store(false, Ordering::SeqCst);
                            return;
                        }
                    };

                    info!(
                        device = %device_name,
                        width,
                        height,
                        ?delivered,
                        "starting MF capture"
                    );

                    let expected = plane_size(delivered, width, height);
                    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
                    while running.load(Ordering::Relaxed) {
                        let mut flags = 0u32;
                        let mut timestamp = 0i64;
                        let mut sample: Option<IMFSample> = None;
                        let result = unsafe {
                            reader.ReadSample(
                                stream,
                                0,
                                None,
                                Some(&mut flags),
                                Some(&mut timestamp),
                                Some(&mut sample),
                            )
                        };
                        if let Err(e) = result {
                            warn!(error = %e, "ReadSample failed");
                            thread::sleep(std::time::Duration::from_millis(5));
                            continue;
                        }
                        if sample.is_none() {
                            continue;
                        }
                        let Some(sample) = sample else { continue };
                        match copy_sample_to_pool(&sample, &pool, expected) {
                            Ok(data) => {
                                let frame = VideoFrame {
                                    captured_at: Instant::now(),
                                    width,
                                    height,
                                    pixel_format: delivered,
                                    data,
                                };
                                if !sender.send_drop_oldest(frame) {
                                    break;
                                }
                            }
                            Err(e) => warn!(error = %e, "sample copy failed"),
                        }
                    }
                    let _ = unsafe { source.Shutdown() };
                    debug!("MF capture thread exit");
                })
                .map_err(|e| CaptureError::Other(Box::new(e)))?,
        );

        match ready_rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => {
                self.running.store(false, Ordering::SeqCst);
                if let Some(handle) = self.worker.take() {
                    let _ = handle.join();
                }
                Err(CaptureError::DeviceOpen(e))
            }
            Err(_) => {
                self.running.store(false, Ordering::SeqCst);
                if let Some(handle) = self.worker.take() {
                    let _ = handle.join();
                }
                Err(CaptureError::DeviceOpen(
                    "timed out opening Media Foundation device".into(),
                ))
            }
        }
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        if !self.running.swap(false, Ordering::SeqCst) {
            return Err(CaptureError::NotRunning);
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
        info!("MF capture stopped");
        Ok(())
    }

    fn receiver(&self) -> FrameReceiver {
        self.receiver.clone()
    }
}

impl Drop for MfVideoSource {
    fn drop(&mut self) {
        if self.running.load(Ordering::SeqCst) {
            let _ = self.stop();
        }
    }
}
