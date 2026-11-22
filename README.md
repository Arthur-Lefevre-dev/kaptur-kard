# CaptureView

Desktop preview app for generic UVC / capture-card sources. Product name **CaptureView**; workspace/binary crate **`captureview`**; repository folder **`kaptur-kard`**.

This repository currently ships **step (a)**: a wgpu window that displays a synthetic `MockVideoSource` (color bars + moving square + frame counter). No real capture card / Media Foundation yet.

## Requirements (Windows)

- Rust stable (1.78+, tested with 1.100)
- A GPU with a working DirectX 12 / Vulkan driver (wgpu)

## Build & run

```powershell
cd c:\Users\kingd\Documents\GitHub\kaptur-kard
cargo build -p captureview
cargo run -p captureview
```

Release (fat LTO, stripped):

```powershell
cargo build --release -p captureview
.\target\release\captureview.exe
```

Tests:

```powershell
cargo test
```

## Step (a) behaviour

- Starts `MockVideoSource` at **1920×1080 @ 60 fps** (BGRA8)
- Bounded frame bus depth **2**, drop-oldest
- Uploads frames to wgpu, presents with **letterboxing**
- Minimal **egui** overlay: FPS + “Mock”

## Workspace layout

| Crate | Role |
|-------|------|
| `crates/capture` | `VideoSource`, formats, modes, `MockVideoSource` |
| `crates/audio` | Stub (cpal wiring later) |
| `crates/render` | wgpu upload / letterbox / present |
| `crates/metrics` | Atomic counters |
| `crates/app` | `captureview` binary (winit + wgpu + egui) |

## Out of scope (v1)

Documented for later work — **not** in this step:

- Recording, streaming, HDR
- H.264/HEVC-only cards
- Proprietary SDKs (DeckLink, etc.)
- DLSS / FSR2 / FSR3 (and FSR1/NIS upscale)
- Full Linux / macOS capture backends (API stubs only)
- Real Media Foundation device capture (planned next)
- Frame interpolation

## Licence

MIT — see [LICENSE](./LICENSE).
