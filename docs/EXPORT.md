# Spout / NDI export (BYO runtimes)

KapturKard can **share the live preview** with OBS, Resolve, and other tools. Runtimes are **not vendored** (licensing / size). Install them yourself, then enable the toggles under **Infos**.

## Spout (Windows)

- **What:** [Spout2](https://spout.zeal.co/) shared-texture sender (GPU path when the DLL is present).
- **Install:** Spout2 release from the Spout project; ensure `SpoutLibrary.dll` is on `PATH` or next to `captureview.exe`.
- **In-app:** Infos → **Spout export**. Sender name defaults to `KapturKard`.
- **OBS:** Add a Spout2 source and pick `KapturKard`.
- **Status:** The Infos panel reports whether the DLL was found. Without it, the toggle stays off / shows a clear message — no console spam.

## NDI (optional)

- **What:** NewTek / Vizrt **NDI** network video.
- **Install:** [NDI Runtime / Tools](https://ndi.video/) (BYO). The app probes `%NDI_RUNTIME_DIR_V6%` / `V5` and `%ProgramFiles%\NDI`.
- **In-app:** Infos → **NDI export**. Sender name defaults to `KapturKard`.
- **License:** Respect NewTek NDI license terms; we do not redistribute NDI binaries.
- **Status:** Scaffolding + probe in v0.3; full send hooks follow once a BYO SDK bridge is linked (`docs/EXPORT.md` stays the source of truth).

## Replay clips vs live export

| Path | Use |
|------|-----|
| **Replay buffer** (F9) | Last *N* seconds → PNG sequence under `%APPDATA%\CaptureView\clips\` |
| **Spout / NDI** | Live frames into OBS / Resolve / network |

```text
ffmpeg -framerate 30 -i frame_%05d.png -c:v libx264 -pix_fmt yuv420p clip.mp4
```
