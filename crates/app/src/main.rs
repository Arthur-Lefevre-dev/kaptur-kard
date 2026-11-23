//! CaptureView step (a): wgpu window + MockVideoSource preview.

// Release Windows builds: no console flash. Debug always keeps a console.
// Opt back in with `--features console` (useful for live stderr while testing release).
// Logs always go to `%APPDATA%\CaptureView\logs\` regardless.
#![cfg_attr(
    all(windows, not(debug_assertions), not(feature = "console")),
    windows_subsystem = "windows"
)]

mod brand;
mod discord_presence;
mod display_caps;
mod export;
mod health;
mod i18n;
mod logging;
mod lut;
mod prefs;
mod profiles;
mod replay;
mod runtime;
mod ui;
mod update_check;

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
