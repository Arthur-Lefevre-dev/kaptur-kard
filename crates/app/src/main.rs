//! CaptureView step (a): wgpu window + MockVideoSource preview.

mod discord_presence;
mod i18n;
mod prefs;
mod runtime;
mod ui;

use anyhow::Context;
use tracing_subscriber::EnvFilter;
use winit::event_loop::{ControlFlow, EventLoop};

use runtime::App;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let event_loop = EventLoop::new().context("create event loop")?;
    // Wait (not Poll): about_to_wait sets WaitUntil for display pacing. Poll spun the
    // CPU with NoVsync (Arthur: ~100 FPS écran + 82% CPU after the 1% low change).
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::default();
    event_loop
        .run_app(&mut app)
        .context("run CaptureView event loop")?;
    Ok(())
}
