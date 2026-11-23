//! KapturKard logo (embedded PNG) for window icon + egui UI.

use egui::{ColorImage, TextureHandle, TextureOptions};
use image::imageops::FilterType;
use winit::window::Icon;

const LOGO_PNG: &[u8] = include_bytes!("../../../assets/kapturkard-logo-256.png");

/// Decode the 256² RGBA logo for egui / winit.
pub fn logo_rgba() -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(LOGO_PNG).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some((w, h, img.into_raw()))
}

fn icon_from_resized(size: u32) -> Option<Icon> {
    let img = image::load_from_memory(LOGO_PNG).ok()?.into_rgba8();
    let resized = image::imageops::resize(&img, size, size, FilterType::Lanczos3);
    let (w, h) = resized.dimensions();
    Icon::from_rgba(resized.into_raw(), w, h).ok()
}

/// Title-bar / Alt-Tab small icon (`ICON_SMALL` on Windows). Prefer 32².
pub fn window_icon() -> Option<Icon> {
    icon_from_resized(32).or_else(|| {
        let (w, h, rgba) = logo_rgba()?;
        Icon::from_rgba(rgba, w, h).ok()
    })
}

/// Taskbar icon (`ICON_BIG` on Windows). 256² matches the brand asset.
pub fn taskbar_icon() -> Option<Icon> {
    let (w, h, rgba) = logo_rgba()?;
    Icon::from_rgba(rgba, w, h).ok()
}

pub fn egui_color_image() -> Option<ColorImage> {
    let (w, h, rgba) = logo_rgba()?;
    Some(ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        &rgba,
    ))
}

/// Load (or reuse) the logo texture on the egui context.
pub fn ensure_logo_texture(ctx: &egui::Context, slot: &mut Option<TextureHandle>) -> Option<TextureHandle> {
    if let Some(t) = slot.as_ref() {
        return Some(t.clone());
    }
    let color = egui_color_image()?;
    let tex = ctx.load_texture("kapturkard-logo", color, TextureOptions::LINEAR);
    *slot = Some(tex.clone());
    Some(tex)
}
