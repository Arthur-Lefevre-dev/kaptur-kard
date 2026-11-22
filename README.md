# CaptureView

Desktop preview app for generic UVC / capture-card sources. Product name **CaptureView**; workspace/binary crate **`captureview`**; repository folder **`kaptur-kard`**.

## Status

**Step (b)** — Windows Media Foundation capture behind `VideoSource`, plus Mock for card-free work.

- Enumerates real video capture devices / modes (MF)
- Prefers native NV12 / YUY2 / BGRA; MJPEG is decoded by MF to NV12 (or RGB32 fallback)
- GPU shader converts YUY2/NV12 → RGB with letterboxing
- UI: device, resolution, Hz, mode (Auto / Game / Quality / Economy)
- No-signal (>1s) and soft-black (>3s) hints
- Frame bus depth **2**, drop-oldest; CPU buffer pool on the capture path

## Requirements (Windows)

- Rust stable (1.78+, tested with 1.100)
- GPU with DirectX 12 / Vulkan (wgpu)
- Optional: UVC / capture card for live MF testing

## Build & run

```powershell
cd c:\Users\kingd\Documents\GitHub\kaptur-kard
cargo build -p captureview
cargo run -p captureview
```

Release:

```powershell
cargo build --release -p captureview
.\target\release\captureview.exe
```

Tests:

```powershell
cargo test --workspace
```

The device list always includes **Mock Color Bars**. Real MF devices appear below (capture-card names ranked first). Default mode is **Auto**.

## Out of scope (v1 / later steps)

- Audio
- Recording, streaming, HDR
- H.264/HEVC-only cards (beyond what MF can decode to NV12/RGB)
- Proprietary SDKs (DeckLink, etc.)
- DLSS / FSR / NIS / interpolation
- Full Linux / macOS capture backends
- DirectShow
- Full consumer chrome polish

## Gaps (step b)

- Some exotic FourCCs are skipped at enum time
- MJPEG relies on MF decoder insertion; if both NV12 and RGB32 output fail, start errors and UI falls back
- RAM budget target &lt;100 Mo @1080p60 without effects is **not** met with the CPU mock path (full BGRA fill); live MF should be lower — see `internal/step-b-report.md` in the Project store

## Licence

MIT — see [LICENSE](./LICENSE).
