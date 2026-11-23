# Spout / NDI export

KapturKard can **share the live preview** with OBS, Resolve, and other tools.

## Spout (Windows) — bundled

- **What:** [Spout2](https://spout.zeal.co/) shared-frame sender (`SpoutLibrary.dll`, BSD-2-Clause).
- **Ship:** Windows builds include `SpoutLibrary.dll` + `spout_bridge.dll` next to `captureview.exe` (also extracted to `%APPDATA%\CaptureView\spout\` on first run if needed).
- **Probe order:** next to exe → `%APPDATA%\CaptureView\spout\` → `third_party/spout` (dev) → `PATH`.
- **In-app:** Infos → **Spout**. Sender name defaults to `KapturKard`.
- **OBS:** Add a Spout2 source and pick `KapturKard`.
- **Send path:** CPU **BGRA `SendImage`** via a thin MIT bridge (`native/spout`). This is real Spout output; a GPU shared-texture path (DX/GL interop with wgpu) is **not** wired yet — the UI says so honestly.
- **License:** SpoutLibrary Simplified BSD — see `licenses/SPOUT-BSD.txt` and `third_party/spout/licence.txt`.

## NDI (optional) — BYO only

- **What:** NewTek / Vizrt **NDI** network video.
- **Install:** [NDI Tools / Runtime](https://ndi.video/) (BYO). **We do not redistribute NDI DLLs.**
- **In-app:** Infos → **NDI setup (download)** opens ndi.video; **Re-check** re-probes; **Open NDI folder** when a runtime folder is found.
- **Probe:** `%NDI_RUNTIME_DIR_V6%` / `V5` / `NDI_RUNTIME_DIR`, then `%ProgramFiles%\NDI`.
- **Status:** Probe + prefs in this release; full send hooks wait on a BYO SDK bridge (`docs/EXPORT.md` stays the source of truth).

## DLSS (BYO NVIDIA) — related setup UX

Mode C / DLSS-G never ship NVIDIA proprietary DLLs. Effects / Infos show a clear greyed reason and buttons:

- **DLSS setup (docs)** → [docs/DLSS.md](./DLSS.md)
- **Open DLSS folder** → `%APPDATA%\CaptureView\dlss5-nr\`

## Replay clips vs live export

| Path | Use |
|------|-----|
| **Replay buffer** (F9) | Last *N* seconds → PNG sequence under `%APPDATA%\CaptureView\clips\` |
| **Spout / NDI** | Live frames into OBS / Resolve / network |

```text
ffmpeg -framerate 30 -i frame_%05d.png -c:v libx264 -pix_fmt yuv420p clip.mp4
```
