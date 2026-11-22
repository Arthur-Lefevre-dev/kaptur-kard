# CaptureView

Desktop preview app for generic UVC / capture-card sources. Product name **CaptureView**; workspace/binary crate **`captureview`**; repository folder **`kaptur-kard`**.

## Status

**Step (f)** — FSR 1 / NIS upscaling.

- Effets: **Off** / **Mode A (FSR 1)** / **Mode B (NIS)** + sharpness; target = on-screen content size
- Greyed when already native resolution or GPU lacks compute (`get_downlevel_capabilities`)
- Auto-disable if frame time repeatedly exceeds budget (~20 ms × 45 frames)
- Adapter vendor/backend logged at startup (DX12/Vulkan preferred; GL noted as fallback)
- Licenses: `licenses/AMD-FSR1-MIT.txt`, `licenses/NVIDIA-NIS-MIT.txt`

## Build & run

```powershell
cd c:\Users\kingd\Documents\GitHub\kaptur-kard
cargo run -p captureview
cargo build --release -p captureview
cargo test --workspace
```

Ideal path: launch → Auto card → full-frame video. Open **Effets** to pick FSR 1 or NIS when the capture is below screen resolution.

## Out of scope (later)

- Optical-flow interpolation (step g)
- DLSS / FSR 2 / FSR 3
- Linux / macOS

## Licence

MIT — see [LICENSE](./LICENSE). Third-party shader ports: see `licenses/`.
