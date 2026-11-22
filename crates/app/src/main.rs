//! CaptureView step (a): wgpu window + MockVideoSource preview.

mod brand;
mod discord_presence;
mod i18n;
mod logging;
mod prefs;
mod runtime;
mod ui;

use anyhow::Context;
use winit::event_loop::{ControlFlow, EventLoop};

use runtime::{App, UserEvent};

fn main() -> anyhow::Result<()> {
    logging::init();

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("create event loop")?;
    // Wait (not Poll): about_to_wait sets WaitUntil for display pacing. Poll spun the
    // CPU with NoVsync (Arthur: ~100 FPS écran + 82% CPU after the 1% low change).
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let mut app = App::with_proxy(proxy);
    event_loop
        .run_app(&mut app)
        .context("run CaptureView event loop")?;
    Ok(())
}
