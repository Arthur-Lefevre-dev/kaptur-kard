//! Synthetic video source: SMPTE-like color bars, moving square, frame counter.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tracing::{debug, info};

use crate::device::{DeviceId, DeviceInfo, FormatDesc};
use crate::format::{PixelFormat, Rational};
use crate::source::{
    frame_channel, CaptureError, FrameReceiver, FrameSender, VideoFrame, VideoSource,
};

const DEFAULT_WIDTH: u32 = 1920;
const DEFAULT_HEIGHT: u32 = 1080;

pub struct MockVideoSource {
    info: DeviceInfo,
    sender: FrameSender,
    receiver: FrameReceiver,
    running: Arc<AtomicBool>,
    frame_index: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
}

impl MockVideoSource {
    pub fn new() -> Self {
        let (sender, receiver) = frame_channel();
        let info = DeviceInfo {
            id: DeviceId::new("mock:color-bars"),
            display_name: "Mock Color Bars".to_string(),
            backend: "mock",
            formats: vec![FormatDesc {
                pixel_format: PixelFormat::Bgra8,
                width: DEFAULT_WIDTH,
                height: DEFAULT_HEIGHT,
                frame_rates: vec![Rational::new(60, 1), Rational::new(30, 1)],
            }],
        };

        Self {
            info,
            sender,
            receiver,
            running: Arc::new(AtomicBool::new(false)),
            frame_index: Arc::new(AtomicU64::new(0)),
            worker: None,
        }
    }
}

impl Default for MockVideoSource {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoSource for MockVideoSource {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn start(&mut self, selected: &FormatDesc) -> Result<(), CaptureError> {
        if self.running.load(Ordering::SeqCst) {
            return Err(CaptureError::AlreadyRunning);
        }
        if selected.pixel_format != PixelFormat::Bgra8 {
            return Err(CaptureError::UnsupportedFormat(format!(
                "{:?}",
                selected.pixel_format
            )));
        }

        let fps = selected
            .frame_rates
            .first()
            .copied()
            .unwrap_or(Rational::new(60, 1));
        let period = Duration::from_nanos(fps.frame_period_ns());
        let width = selected.width;
        let height = selected.height;

        self.running.store(true, Ordering::SeqCst);
        self.frame_index.store(0, Ordering::SeqCst);

        let running = Arc::clone(&self.running);
        let sender = self.sender.clone();
        let frame_index = Arc::clone(&self.frame_index);

        info!(
            width,
            height,
            fps = fps.as_f64(),
            "starting MockVideoSource"
        );

        self.worker = Some(thread::Builder::new()
            .name("mock-capture".into())
            .spawn(move || {
                let start = Instant::now();
                let mut next = Instant::now();
                while running.load(Ordering::Relaxed) {
                    let now = Instant::now();
                    if now < next {
                        thread::sleep(next - now);
                    }
                    next += period;

                    let idx = frame_index.fetch_add(1, Ordering::Relaxed);
                    let data = render_mock_frame(width, height, idx);
                    let frame = VideoFrame {
                        timestamp_ns: start.elapsed().as_nanos() as u64,
                        width,
                        height,
                        pixel_format: PixelFormat::Bgra8,
                        data,
                    };
                    if !sender.send_drop_oldest(frame) {
                        debug!("mock frame channel disconnected");
                        break;
                    }
                }
            })
            .map_err(|e| CaptureError::Other(Box::new(e)))?);

        Ok(())
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        if !self.running.swap(false, Ordering::SeqCst) {
            return Err(CaptureError::NotRunning);
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
        info!("MockVideoSource stopped");
        Ok(())
    }

    fn receiver(&self) -> FrameReceiver {
        self.receiver.clone()
    }
}

impl Drop for MockVideoSource {
    fn drop(&mut self) {
        if self.running.load(Ordering::SeqCst) {
            let _ = self.stop();
        }
    }
}

/// SMPTE-like color bars + bouncing square + decimal frame counter in BGRA8.
fn render_mock_frame(width: u32, height: u32, frame_idx: u64) -> Arc<[u8]> {
    let w = width as usize;
    let h = height as usize;
    let mut buf = vec![0u8; w * h * 4];

    // Seven vertical color bars (BGRA).
    let bars: [(u8, u8, u8); 7] = [
        (192, 192, 192), // white-ish
        (0, 192, 192),   // yellow
        (192, 192, 0),   // cyan
        (0, 192, 0),     // green
        (192, 0, 192),   // magenta
        (0, 0, 192),     // red
        (192, 0, 0),     // blue
    ];
    let bar_w = (w / bars.len()).max(1);

    for y in 0..h {
        for x in 0..w {
            let bar = (x / bar_w).min(bars.len() - 1);
            let (b, g, r) = bars[bar];
            let i = (y * w + x) * 4;
            buf[i] = b;
            buf[i + 1] = g;
            buf[i + 2] = r;
            buf[i + 3] = 255;
        }
    }

    // Moving square (horizontal bounce).
    let square = (h / 8).max(16);
    let travel = (w.saturating_sub(square)).max(1);
    let period = (travel * 2) as u64;
    let phase = (frame_idx as usize) % period as usize;
    let sq_x = if phase < travel {
        phase
    } else {
        travel * 2 - phase
    };
    let sq_y = h / 2 - square / 2;
    fill_rect(&mut buf, w, h, sq_x, sq_y, square, square, (40, 40, 40));

    // Frame counter digits (top-left).
    draw_number(&mut buf, w, h, 16, 16, frame_idx, (255, 255, 255));

    Arc::from(buf.into_boxed_slice())
}

fn fill_rect(
    buf: &mut [u8],
    stride_w: usize,
    h: usize,
    x: usize,
    y: usize,
    rw: usize,
    rh: usize,
    bgr: (u8, u8, u8),
) {
    let (b, g, r) = bgr;
    for yy in y..(y + rh).min(h) {
        for xx in x..(x + rw).min(stride_w) {
            let i = (yy * stride_w + xx) * 4;
            buf[i] = b;
            buf[i + 1] = g;
            buf[i + 2] = r;
            buf[i + 3] = 255;
        }
    }
}

/// Tiny 3x5 digit font for the frame counter.
fn draw_number(
    buf: &mut [u8],
    stride_w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    value: u64,
    bgr: (u8, u8, u8),
) {
    let text = format!("{value}");
    let scale = 4usize;
    let mut x = x0;
    for ch in text.chars() {
        if let Some(glyph) = digit_glyph(ch) {
            for row in 0..5 {
                for col in 0..3 {
                    if glyph[row] & (1 << (2 - col)) != 0 {
                        fill_rect(
                            buf,
                            stride_w,
                            h,
                            x + col * scale,
                            y0 + row * scale,
                            scale,
                            scale,
                            bgr,
                        );
                    }
                }
            }
        }
        x += 4 * scale;
    }
}

fn digit_glyph(ch: char) -> Option<[u8; 5]> {
    // 3-bit rows, MSB = left pixel.
    Some(match ch {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::TryRecvError;

    #[test]
    fn mock_produces_bgra_frames() {
        let mut source = MockVideoSource::new();
        let fmt = source.info().formats[0].clone();
        source.start(&fmt).expect("start");
        let rx = source.receiver();

        let mut got = None;
        for _ in 0..50 {
            match rx.try_recv() {
                Ok(frame) => {
                    got = Some(frame);
                    break;
                }
                Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
                Err(TryRecvError::Disconnected) => panic!("disconnected"),
            }
        }
        let frame = got.expect("expected a mock frame");
        assert_eq!(frame.pixel_format, PixelFormat::Bgra8);
        assert_eq!(frame.width, 1920);
        assert_eq!(frame.height, 1080);
        assert_eq!(frame.data.len(), 1920 * 1080 * 4);
        source.stop().expect("stop");
    }
}
