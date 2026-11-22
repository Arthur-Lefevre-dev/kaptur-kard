# NVIDIA Streamline / DLSS (Mode C)

CaptureView Mode **C — DLSS** targets the official **NVIDIA Streamline** SDK with the **DLSS Super Resolution** plugin (current public NGX / DLSS 4.x transformer presets). There is **no separate public “DLSS 5” SDK package** as of this write-up; use the latest Streamline release zip.

## Why binaries are not in git

NVIDIA RTX / Streamline redistributables are under the [NVIDIA RTX SDKs license](https://github.com/NVIDIA/DLSS/blob/main/LICENSE.txt). Signed DLLs (`sl.interposer.dll`, `nvngx_dlss.dll`, …) ship in the **GitHub release zip**, not in the Streamline source tree. We **do not vendor** those DLLs in this repository — Arthur must download them once and point the app at the folder.

## Install on Windows (Arthur)

1. Open [NVIDIA-RTX/Streamline releases](https://github.com/NVIDIA-RTX/Streamline/releases).
2. Download the production package for **v2.14.16** (or newer 2.14.x) — e.g. the published SDK zip.
3. Accept the NVIDIA license when prompted / as stated in the package.
4. Unpack to a stable path, for example:
   `C:\SDK\Streamline-2.14.16`
5. Confirm you can see `sl.interposer.dll` and `nvngx_dlss.dll` (often under `bin\x64\` depending on the zip layout).
6. Set a user or session environment variable:

```powershell
$env:STREAMLINE_SDK_PATH = "C:\SDK\Streamline-2.14.16"
# optional alias:
# $env:CAPTUREVIEW_STREAMLINE_DIR = $env:STREAMLINE_SDK_PATH
```

7. Build with the feature flag:

```powershell
cd C:\Users\kingd\Documents\GitHub\kaptur-kard   # or your worktree
cargo run -p captureview --features dlss
# release:
cargo build --release -p captureview --features dlss
```

8. In CaptureView → Effects → **Mode C — DLSS**.

### Alternate layout (no env var)

Copy the Streamline `bin\x64` (or root) contents next to the exe under `streamline\` or `third_party\streamline\`.

## Graceful fallback

| Situation | Behaviour |
|-----------|-----------|
| Default build (no `--features dlss`) | Mode C selectable; status explains feature flag; **NIS** runs |
| AMD / Intel GPU | Mode C → **NIS** fallback |
| OpenGL backend | Mode C → **NIS** fallback (need DX12/Vulkan) |
| Feature on, SDK path missing | Mode C → **NIS** fallback |
| SDK DLLs loaded | Probe succeeds (`SdkPresentPendingHook`); **NIS** still used until native evaluate is wired |
| FSR 1 / NIS | Unchanged |

## Architecture notes

- Upscale pipeline: upload → color → deinterlace → interpolate → **upscale (Off / FSR1 / NIS / DLSS)** → present.
- Streamline needs a **native D3D12 or Vulkan device**. CaptureView presents through **wgpu**, which does not currently expose the underlying `ID3D12Device` / `VkDevice` for `slSetD3DDevice` / `slSetVulkanInfo`.
- Until that hook exists, Mode C **always falls back to the existing NIS compute path** so picture enhancement never goes blank.
- Full game-style DLSS SR also expects motion vectors, depth, and jitter — a capture preview may later use a limited / DLAA-style path once the device hook lands.

## Blockers (honest)

1. **License / download** — Arthur must fetch the Streamline release zip; we cannot commit NVIDIA-signed NGX DLLs into MIT git history without that install step.
2. **wgpu native device** — no production `slEvaluateFeature` path yet; probe + UI + NIS fallback only.
3. **“DLSS 5”** — marketing name not mirrored by a distinct public SDK; track Streamline **2.14.x** / NGX from NVIDIA developer releases.

## References

- Streamline repo: https://github.com/NVIDIA-RTX/Streamline
- Programming guide (in SDK / docs on GitHub)
- DLSS overview: https://developer.nvidia.com/rtx/dlss
