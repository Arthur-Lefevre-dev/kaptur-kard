//! Linux V4L2 capture behind [`crate::source::VideoSource`].
//!
//! ## Build on Linux
//!
//! ```bash
//! cargo build -p capture --target x86_64-unknown-linux-gnu
//! cargo run -p captureview
//! ```
//!
//! Requires a V4L2-capable kernel and (for the `v4l` crate) libv4l development
//! headers when linking against system libraries. Runtime probing walks
//! `/dev/video0`…`/dev/video15`.
//!
//! This module is compiled only under `cfg(target_os = "linux")`; Windows CI
//! never builds it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tracing::{info, warn};
use v4l::buffer::Type as BufferType;
use v4l::io::traits::CaptureStream;
use v4l::prelude::MmapStream;
use v4l::video::Capture;
use v4l::{Device, Format, FourCC};

use crate::device::{DeviceId, DeviceInfo, FormatDesc};
use crate::format::{looks_interlaced, PixelFormat, Rational};
use crate::pool::BufferPool;
use crate::source::{
    frame_channel, CaptureError, FrameReceiver, FrameSender, VideoFrame, VideoSource,
};

/// Enumerate `/dev/video*` nodes that advertise `VIDEO_CAPTURE`.
pub fn enumerate_devices() -> Result<Vec<DeviceInfo>, CaptureError> {
    let mut out = Vec::new();
    for idx in 0..16usize {
        let path = format!("/dev/video{idx}");
        let Ok(dev) = Device::with_path(&path) else {
            continue;
        };
        let Ok(caps) = dev.query_caps() else {
            continue;
        };
        if !caps
            .capabilities
            .contains(v4l::capability::Flags::VIDEO_CAPTURE)
        {
            continue;
        }
        let formats = probe_formats(&dev).unwrap_or_else(|e| {
            warn!(%path, error = %e, "v4l2 format probe failed; using fallback presets");
            fallback_formats()
        });
        out.push(DeviceInfo {
            id: DeviceId::new(path.clone()),
            display_name: format!("{} ({path})", caps.card),
            backend: "v4l2",
            formats,
        });
    }
    Ok(out)
}

fn fourcc_to_pixel(fourcc: FourCC) -> Option<PixelFormat> {
    match &fourcc.repr {
        b"YUYV" | b"YUY2" => Some(PixelFormat::Yuy2),
        b"NV12" => Some(PixelFormat::Nv12),
        b"MJPG" | b"JPEG" => Some(PixelFormat::Mjpeg),
        b"RGB3" | b"BGR3" => Some(PixelFormat::Rgb24),
        b"BA24" | b"AR24" => Some(PixelFormat::Bgra8),
        _ => None,
    }
}

fn pixel_to_fourcc(fmt: PixelFormat) -> FourCC {
    match fmt {
        PixelFormat::Yuy2 => FourCC::new(b"YUYV"),
        PixelFormat::Nv12 => FourCC::new(b"NV12"),
        PixelFormat::Mjpeg => FourCC::new(b"MJPG"),
        PixelFormat::Rgb24 => FourCC::new(b"RGB3"),
        PixelFormat::Bgra8 => FourCC::new(b"AR24"),
        PixelFormat::Unknown(_) => FourCC::new(b"YUYV"),
    }
}

fn fallback_formats() -> Vec<FormatDesc> {
    vec![
        FormatDesc {
            pixel_format: PixelFormat::Yuy2,
            width: 1280,
            height: 720,
            frame_rates: vec![Rational::new(30, 1)],
            interlaced: false,
        },
        FormatDesc {
            pixel_format: PixelFormat::Yuy2,
            width: 720,
            height: 480,
            frame_rates: vec![Rational::new(30000, 1001)],
            interlaced: true,
        },
    ]
}

fn probe_formats(dev: &Device) -> Result<Vec<FormatDesc>, CaptureError> {
    let mut out = Vec::new();
    let descriptions = Capture::enum_formats(dev)
        .map_err(|e| CaptureError::Negotiate(format!("v4l2 enum_formats: {e}")))?;
    for desc in descriptions {
        let Some(pixel_format) = fourcc_to_pixel(desc.fourcc) else {
            continue;
        };
        let sizes = Capture::enum_framesizes(dev, desc.fourcc).unwrap_or_default();
        for size in sizes {
            match size.size {
                v4l::framesize::FrameSizeEnum::Discrete(d) => {
                    push_size_rates(
                        &mut out,
                        dev,
                        desc.fourcc,
                        pixel_format,
                        d.width,
                        d.height,
                    );
                }
                v4l::framesize::FrameSizeEnum::Stepwise(s) => {
                    // Sample a few progressive points in the stepwise range.
                    for &(w, h) in &[
                        (s.min_width, s.min_height),
                        (s.max_width.min(1920), s.max_height.min(1080)),
                        (1280u32.min(s.max_width), 720u32.min(s.max_height)),
                    ] {
                        if w >= s.min_width && h >= s.min_height {
                            push_size_rates(&mut out, dev, desc.fourcc, pixel_format, w, h);
                        }
                    }
                }
            }
        }
    }
    if out.is_empty() {
        return Err(CaptureError::Negotiate("no usable v4l2 formats".into()));
    }
    Ok(out)
}

fn push_size_rates(
    out: &mut Vec<FormatDesc>,
    dev: &Device,
    fourcc: FourCC,
    pixel_format: PixelFormat,
    width: u32,
    height: u32,
) {
    let intervals = Capture::enum_frameintervals(dev, fourcc, width, height).unwrap_or_default();
    let mut rates = Vec::new();
    for iv in intervals {
        match iv.interval {
            v4l::frameinterval::FrameIntervalEnum::Discrete(frac) => {
                if frac.numerator > 0 {
                    // Frame interval is seconds per frame; fps = den/num.
                    rates.push(Rational::new(frac.denominator, frac.numerator.max(1)));
                }
            }
            v4l::frameinterval::FrameIntervalEnum::Stepwise(s) => {
                if s.min.numerator > 0 {
                    rates.push(Rational::new(s.min.denominator, s.min.numerator.max(1)));
                }
                if s.max.numerator > 0 {
                    rates.push(Rational::new(s.max.denominator, s.max.numerator.max(1)));
                }
            }
        }
    }
    if rates.is_empty() {
        rates.push(Rational::new(30, 1));
    }
    for rate in rates {
        out.push(FormatDesc {
            pixel_format,
            width,
            height,
            frame_rates: vec![rate],
            interlaced: looks_interlaced(width, height, rate),
        });
    }
}

pub struct V4l2VideoSource {
    info: DeviceInfo,
    sender: FrameSender,
    receiver: FrameReceiver,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    pool: Arc<BufferPool>,
}

impl V4l2VideoSource {
    pub fn open(info: DeviceInfo) -> Result<Self, CaptureError> {
        let (sender, receiver) = frame_channel();
        Ok(Self {
            info,
            sender,
            receiver,
            running: Arc::new(AtomicBool::new(false)),
            worker: None,
            pool: BufferPool::new(),
        })
    }
}

impl VideoSource for V4l2VideoSource {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn start(&mut self, selected: &FormatDesc) -> Result<(), CaptureError> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(CaptureError::AlreadyRunning);
        }
        let path = self.info.id.as_str().to_string();
        let width = selected.width.max(1);
        let height = selected.height.max(1);
        let period = selected
            .frame_rates
            .first()
            .copied()
            .unwrap_or(Rational::new(30, 1))
            .frame_period_ns();
        let pixel_format = selected.pixel_format;
        let running = self.running.clone();
        let sender = self.sender.clone();
        let pool = self.pool.clone();

        self.worker = Some(
            thread::Builder::new()
                .name("v4l2-capture".into())
                .spawn(move || {
                    let Ok(mut dev) = Device::with_path(&path) else {
                        warn!(%path, "failed to open v4l2 device");
                        return;
                    };
                    let fourcc = pixel_to_fourcc(pixel_format);
                    let fmt = Format::new(width, height, fourcc);
                    match Capture::set_format(&dev, &fmt) {
                        Ok(applied) => info!(
                            %path,
                            width = applied.width,
                            height = applied.height,
                            "v4l2 format applied"
                        ),
                        Err(e) => warn!(error = %e, "v4l2 set_format failed; using device default"),
                    }

                    match MmapStream::with_buffers(&mut dev, BufferType::VideoCapture, 4) {
                        Ok(mut stream) => {
                            info!(%path, "starting V4L2 MMAP capture");
                            while running.load(Ordering::Relaxed) {
                                match stream.next() {
                                    Ok((buf, _meta)) => {
                                        let len = buf.len();
                                        let mut pooled = pool.take(len.max(1));
                                        let n = len.min(pooled.len());
                                        pooled.as_mut_slice()[..n].copy_from_slice(&buf[..n]);
                                        let frame = VideoFrame {
                                            captured_at: Instant::now(),
                                            width,
                                            height,
                                            pixel_format,
                                            data: pooled.into_arc(),
                                        };
                                        if !sender.send_drop_oldest(frame) {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "v4l2 stream next failed");
                                        thread::sleep(Duration::from_millis(5));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                error = %e,
                                "v4l2 mmap stream unavailable; emitting placeholder frames"
                            );
                            let mut next = Instant::now();
                            while running.load(Ordering::Relaxed) {
                                let now = Instant::now();
                                if now < next {
                                    thread::sleep(next - now);
                                }
                                next += Duration::from_nanos(period);
                                let bytes = (width * height * 2) as usize;
                                let data = pool.take(bytes).into_arc();
                                let frame = VideoFrame {
                                    captured_at: Instant::now(),
                                    width,
                                    height,
                                    pixel_format: PixelFormat::Yuy2,
                                    data,
                                };
                                if !sender.send_drop_oldest(frame) {
                                    break;
                                }
                            }
                        }
                    }
                })
                .map_err(|e| CaptureError::Other(Box::new(e)))?,
        );
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        if !self.running.swap(false, Ordering::SeqCst) {
            return Err(CaptureError::NotRunning);
        }
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        Ok(())
    }

    fn receiver(&self) -> FrameReceiver {
        self.receiver.clone()
    }
}
