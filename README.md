# CaptureView

Desktop preview app for generic UVC / capture-card sources. Product / window name **KapturKard**; workspace/binary crate **`captureview`**; repository folder **`kaptur-kard`**. Brand mark: `assets/kapturkard-logo.png` (Windows `.ico` embedded in the exe).

## Status

**Step (h)** — extended compatibility (last planned brief step).

| Area | Status |
|------|--------|
| Bob deinterlace (Auto / Off / Bob) | **Done** — Auto when mode is interlaced (480i / 576i / 1080i) |
| Windows Media Foundation | **Done** — primary capture path |
| DirectShow fallback | **Partial** — feature-gated scaffolding (`--features directshow`); empty enum until DS graph lands |
| Linux V4L2 | **Done (structure)** — real `v4l` crate list/start/stop; build/test on Linux |
| macOS AVFoundation | **Stub** — `cfg(macos)` scaffolding; compile on Apple hosts |
| Hot-plug (~2 s poll) | **Done** — reconnect last settings by device id |
| Frame interp / FSR1 / NIS / audio / metrics | **Done** (steps c–g) |

## Platform matrix

| OS | Video backend | Build notes |
|----|---------------|-------------|
| Windows 10/11 | Media Foundation (primary) | `cargo run -p captureview` — tested host for this project |
| Windows | DirectShow (optional) | `cargo run -p captureview --features directshow` — scaffolding only |
| Linux | V4L2 (`v4l` crate) | `cargo build -p capture` / `cargo run -p captureview` on a Linux host with `/dev/video*` |
| macOS | AVFoundation scaffolding | Compiles on Apple targets; capture start returns Unsupported until AVF I/O is wired |

Mock source is always available for CI and card-free development.

## Build & run

```powershell
cd c:\Users\kingd\Documents\GitHub\kaptur-kard
cargo run -p captureview
cargo build --release -p captureview
cargo test --workspace
```

## Logs

CaptureView writes **tracing** logs to stderr and to a daily rotating file:

`%APPDATA%\CaptureView\logs\captureview.YYYY-MM-DD.log`

- Default level: **info** (Infos tab can change it; persisted in `ui.toml`).
- Override with `RUST_LOG` (e.g. `RUST_LOG=debug`).
- **Copy report for support** includes the log path and a tail of the file.
- Older files are pruned (keeps ~14 days).

### Linux

```bash
cargo build -p capture
cargo run -p captureview
# Needs a V4L2-capable system; libv4l headers if linking system libs.
```

### macOS

```bash
cargo build -p capture --target aarch64-apple-darwin
# On an Apple host: cargo run -p captureview
# Capture remains scaffolding until AVFoundation is implemented.
```

## Effects pipeline

upload → color convert → **deinterlace (Bob)** → **interpolate** → **upscale (FSR1 / NIS)** → present

- **Deinterlace**: Auto (default) enables Bob when the active format is flagged interlaced; user can force Off or Bob.
- **Interpolation**: Off / x2 / Auto (~one source frame of added delay when on).
- **Upscale**: Off / FSR 1 / NIS toward the on-screen letterbox size.

## Lag measurement method

Software cannot measure true end-to-end input lag (controller → console → HDMI → panel). CaptureView reports:

1. **Pipeline delay** — capture timestamp → present (app-internal).
2. **Manual lag test (Info / F2)** — white flash + human stopwatch helper; not a hardware measurement.

Use an external LED / high-speed camera / dedicated lag tester for absolute HDMI lag.

## Tested cards / limits

| Item | Notes |
|------|--------|
| Host | Windows (this repo’s primary CI/dev machine) |
| Mock | Always works (synthetic YUY2) |
| UVC / capture cards via MF | Enumerated when Windows exposes them to Media Foundation |
| Interlaced | Heuristic + MF interlace flag; Bob GPU path needs compute shaders |
| DirectShow-only cards | Not captured yet — enable feature for future work; prefer MF |
| Linux / macOS capture | Code present; runtime verification belongs on those OS hosts |

## Out of scope

- Full DirectShow sample-grabber graph
- Production AVFoundation capture
- DLSS / FSR 2 / FSR 3
- Hardware lag meters

## What’s next

1. Finish DirectShow for MF-missing Windows cards (real filter graph).
2. Complete macOS AVFoundation device I/O.
3. Expand Linux V4L2 format probing / MJPEG decode path on real hardware.
4. Optional: field-aware deinterlace (weave / motion-adaptive) beyond Bob.
5. Add a GitHub remote for `kaptur-kard` when ready (`git remote add origin …`) — local commits are enough until then; no PR without a remote.

## Licence

MIT — see [LICENSE](./LICENSE). Third-party shader ports: see `licenses/`.
