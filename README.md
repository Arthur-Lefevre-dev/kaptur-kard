# CaptureView

Desktop preview app for generic UVC / capture-card sources. Product name **CaptureView**; workspace/binary crate **`captureview`**; repository folder **`kaptur-kard`**.

## Status

**Step (g)** — optical-flow frame interpolation (x2).

- Effets → **Interpolation**: Off / **x2** / **Auto** (keeps x2 only while the GPU stays within budget)
- Adds ~**one source frame** of display delay (shown in the UI)
- Pipeline: upload → color convert → **interpolate** → upscale → present (hot-toggleable)
- Same performance guard as upscale: repeated frame-budget misses → auto-disable + French notice
- Greyed when compute shaders are unavailable

### Quality vs latency

| Mode | Motion | Added delay | GPU cost |
|------|--------|-------------|----------|
| Off | Source cadence | None | Baseline |
| x2 | Smoother (inserted mid-frame) | ~1 source frame | Flow + warp every new capture |
| Auto | Like x2 while budget holds | Same as x2 when on | Auto-off if too heavy |

Spatial upscale (FSR 1 / NIS) can still run **after** interpolation.

## Build & run

```powershell
cd c:\Users\kingd\Documents\GitHub\kaptur-kard
cargo run -p captureview
cargo build --release -p captureview
cargo test --workspace
```

## Out of scope (later)

- Deinterlace polish / DirectShow / Linux V4L2 / macOS (step h)
- DLSS / FSR 2 / FSR 3

## Licence

MIT — see [LICENSE](./LICENSE). Third-party shader ports: see `licenses/`.
