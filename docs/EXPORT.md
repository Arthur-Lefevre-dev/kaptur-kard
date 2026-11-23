# Spout / NDI export

KapturKard can **share the live preview** with OBS, Resolve, and other tools.

## Spout (Windows) — bundled

- **What:** [Spout2](https://spout.zeal.co/) shared-frame sender (`SpoutLibrary.dll`, BSD-2-Clause).
- **Ship:** Windows builds include `SpoutLibrary.dll` + `spout_bridge.dll` next to `captureview.exe` (also extracted to `%APPDATA%\CaptureView\spout\` on first run if needed).
- **Probe order:** next to exe → `%APPDATA%\CaptureView\spout\` → `third_party/spout` (dev) → `PATH`.
- **Default:** **Off** (Spout adds ~30 ms display delay). Existing installs are migrated off once; re-enable in Infos when needed.
- **In-app:** Infos → **Spout**. Sender name defaults to `KapturKard`.
- **OBS:** Add a Spout2 source and pick `KapturKard`.
- **Send path:** CPU **BGRA `SendImage`** via a thin MIT bridge (`native/spout`). Live **NV12 / YUY2** capture is converted to BGRA before send (MF cards usually deliver NV12). A GPU shared-texture path (DX/GL interop with wgpu) remains optional for later.
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
| **Replay buffer** (Clips tab, F9) | Off by default. Last *N* seconds (NV12 ring up to ~1080p, ~20 fps) → background **H.264 MP4** (CRF 18) via ffmpeg when present, else **MJPEG AVI**, under `%APPDATA%\CaptureView\clips\` |
| **Clips tab** | Replay options + list / play / reveal / delete saved clips (Infos keeps monitoring / export / health / updates) |
| **Spout / NDI** | Live frames into OBS / Resolve / network (off by default) |
