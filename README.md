# CaptureView

Desktop preview app for generic UVC / capture-card sources. Product name **CaptureView**; workspace/binary crate **`captureview`**; repository folder **`kaptur-kard`**.

## Status

**Step (c)** — audio in/out on top of MF video + Mock.

- Video: Media Foundation + Mock (step b)
- Audio: `cpal` capture/playback, `rubato` resample, `ringbuf` bus
- Input list starts with **Aucun son**; USB VID/PID / name heuristics associate capture-card audio when possible
- Output defaults to system default; remembered in `%APPDATA%\CaptureView\audio.toml`; missing device → French fallback notice
- Volume, mute, level meter; Advanced: delay 0–500 ms, buffer 20–40 ms
- Soft clock-drift correction from ring fill

## Requirements (Windows)

- Rust stable (1.78+, tested with 1.100)
- GPU with DirectX 12 / Vulkan (wgpu)
- Optional: UVC / capture card + its USB audio endpoint

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

### Enable sound

1. Bottom **Son** bar → choose **Source du son** (or keep **Aucun son** for silent / Mock)
2. Choose output (default system device is marked)
3. Uncheck **Muet**, set **Vol**; watch **Niveau**
4. **Avancé**: audio delay (ms) to resync with video; buffer 20–40 ms

Prefs persist under `%APPDATA%\CaptureView\audio.toml`.

Tests:

```powershell
cargo test --workspace
```

## Out of scope (later)

- Recording, streaming, HDR
- FSR / NIS / interpolation
- Full i18n / consumer wizard (step d)
- Linux / macOS audio/video backends
- True Windows ContainerId SetupAPI walk (heuristic VID/PID + name for now)

## Licence

MIT — see [LICENSE](./LICENSE).
