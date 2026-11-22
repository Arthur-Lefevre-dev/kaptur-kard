# CaptureView

Desktop preview app for generic UVC / capture-card sources. Product name **CaptureView**; workspace/binary crate **`captureview`**; repository folder **`kaptur-kard`**.

## Status

**Step (e)** — metrics simple / detailed.

- Full-frame video with floating toolbar (Image / Son / Effets / Infos / Plein écran)
- **F2** cycles metrics overlay: Simple (FPS + display delay) → Detailed (frame-time graph, 1% low, drops, capture vs display FPS, CPU/RAM, GPU, active format) → Off
- Display delay = capture timestamp → present (not true input lag — software cannot measure controller/HDMI/panel lag)
- Infos tab shows the same live metrics + optional **manual lag test** (screen flash + stopwatch)
- i18n FR/EN via `sys-locale` + `locales/{fr,en}.toml`

## Build & run

```powershell
cd c:\Users\kingd\Documents\GitHub\kaptur-kard
cargo run -p captureview
cargo build --release -p captureview
cargo test --workspace
```

Ideal path: launch → card selected automatically (Auto) → picture fills the window. Press **F2** for metrics.

### Manual lag test (helper only)

In **Infos**: **Démarrer le flash** whites the window briefly; stop the timer when you see the flash. This is a human stopwatch, not a hardware lag measurement. Real input lag requires external tools (high-speed camera, Leo Bodnar, etc.).

## Out of scope (later)

- Real FSR / NIS / interpolation shaders
- Linux / macOS

## Licence

MIT — see [LICENSE](./LICENSE).
