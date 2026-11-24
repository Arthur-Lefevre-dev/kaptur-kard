# Spout / NDI export

KapturKard can **share the live preview** with OBS, Resolve, and other tools.

## Spout (Windows) — bundled

- **What:** [Spout2](https://spout.zeal.co/) shared-frame sender (`SpoutLibrary.dll`, BSD-2-Clause).
- **Ship:** Windows builds include `SpoutLibrary.dll` + `spout_bridge.dll` next to `captureview.exe` (also extracted to `%APPDATA%\CaptureView\spout\` on first run if needed).
- **Probe order:** next to exe → `%APPDATA%\CaptureView\spout\` → `third_party/spout` (dev) → `PATH`.
- **Default:** **Off**. Existing installs are migrated off once; re-enable in Infos when needed.
- **In-app:** Infos → **Spout**. Sender name defaults to `KapturKard`.
- **OBS:** Add a Spout2 source and pick `KapturKard`.
- **Send path (v0.3.8+):** Capture publishes the latest `Arc` frame to a **background worker**. The worker does NV12/YUY2→BGRA (integer, scratch-reused) and `SendImage`, dropping intermediates so OBS lag cannot stall present. The UI/present thread no longer converts or calls Spout synchronously.
- **Send FPS (v0.3.9+):** User-selectable **30 / 60 / 120** (default **60**) next to the Spout checkbox. Previously a fixed ~125 Hz cap. Intermediate frames are still dropped when OBS cannot keep up.
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
| **Replay buffer** (Clips tab, F9) | Off by default. Last *N* seconds (NV12 ring up to ~1080p, **30 / 60 / 120 fps** sample — default 30) + stereo PCM from the monitor path → background **H.264+AAC MP4** (CRF 18) via ffmpeg when present (`CREATE_NO_WINDOW` on Windows), else **MJPEG AVI** (video-only), under `%APPDATA%\CaptureView\clips\` |
| **Clips tab** | Replay options + FPS choice + list / play / reveal / delete with first-frame thumbs; encode FPS shown vs capture FPS. Clip toast is top-right. |
| **Spout / NDI** | Live frames into OBS / Resolve / network (off by default). Spout send rate selectable **30 / 60 / 120**. |

## Audio notes (live cutscene dialogue)

Live monitoring (WASAPI via cpal) prefers a **stereo** input config when the capture card exposes one, so multi-channel fold does not unnecessarily duck levels. True stereo is passthrough; multi-channel fold keeps **center at full level** and does not apply a global headroom duck when only FL/FR carry energy. Live volume boost goes up to **300%** with soft-clip (clips still record pre-volume PCM).

### NZXT Signal HD60 vs Switch 2 HDMI audio

| Source | Official audio |
|--------|----------------|
| **NZXT Signal HD60** | HDMI capture: **2 ch stereo, 16-bit, 48 kHz** only ([manual](https://cdn-g.nzxt.com/dl/1676419258-streaming_sprite_signal_hd60_digital-manual_v2_en_2211.pdf)) |
| **Nintendo Switch 2** (TV/dock) | HDMI: **Linear PCM** — System Settings → Audio → TV Sound: Automatic / **Stereo** / **Linear PCM 5.1 Surround** ([Nintendo specs](https://www.nintendo.com/us/gaming-systems/switch-2/tech-specs/)) |

**Conclusion:** Quiet live cutscene voices with headset-on-Switch OK are usually **card / HDMI mix**, not a KapturKard center drop. The HD60 cannot capture discrete 5.1; if Switch 2 is set to LPCM 5.1, the card’s stereo path may keep only FL/FR and under-represent center dialogue. Prefer **TV Sound → Stereo** on Switch 2 when capturing with HD60. In-app Sound shows live **channels + Hz**, **output mode (Stereo / 5.1)**, and an explicit **Downmix 5.1 → Stereo** (full center, default ON). Stereo-only cards also get a hint banner.

**How to verify if voices stay quiet after this release:**

1. In Sound, check **Capture audio** (channels · Hz). HD60 should read Stereo · 48000 Hz.
2. On Switch 2, set **TV Sound → Stereo**, then retest a cutscene.
3. Raise Volume above 100% during a cutscene — dialogue should lift on the live path when the channel is present.
4. Compare the same HDMI feed in the capture card’s own preview app / TV. If dialogue is also quiet there, the mix is **game or card**, not KapturKard.
5. Check logs for `preferring stereo over multi-channel` / `input_channels=` when the engine starts.
