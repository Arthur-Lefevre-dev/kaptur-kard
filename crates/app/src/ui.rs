//! Minimal egui overlay (FPS + mock label).

use egui::{Context, RichText};

pub struct OverlayState {
    pub fps: f32,
    pub label: String,
    pub frame_w: u32,
    pub frame_h: u32,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            fps: 0.0,
            label: "Mock".to_string(),
            frame_w: 0,
            frame_h: 0,
        }
    }
}

pub fn draw_overlay(ctx: &Context, state: &OverlayState) {
    egui::Area::new(egui::Id::new("captureview_overlay"))
        .fixed_pos(egui::pos2(12.0, 12.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("CaptureView — Mock")
                            .color(egui::Color32::WHITE)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(format!("{:.0} FPS · {}", state.fps, state.label))
                            .color(egui::Color32::LIGHT_GRAY),
                    );
                    if state.frame_w > 0 {
                        ui.label(
                            RichText::new(format!("{}×{}", state.frame_w, state.frame_h))
                                .color(egui::Color32::GRAY),
                        );
                    }
                });
        });
}
