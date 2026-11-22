# DLSS 5 Neural Rendering (Mode C)

Mode **C** runs **NVIDIA DLSS 5 Neural Rendering** — NGX **feature 18** via `nvngx_dlssnr.dll` — on CaptureView video frames. This is the same family of path used by community tools such as [obs-dlss5-nr](https://github.com/Saganaki22/obs-dlss5-nr), Glasir, and ComfyUI-DLSS5-NR.

It is **not** classic game DLSS Super Resolution (Streamline DLSS-SR). NR is a same-resolution neural enhance / “relight” of the frame.

## GPU requirements

| GPU | Status |
|-----|--------|
| **RTX 50-series** | Officially supported by NVIDIA for DLSS Neural Rendering |
| RTX 40 / older | Often `FeatureNotSupported` (`0xBAD00001`) with stock runtimes |
| AMD / Intel | Not supported |

Driver: **616.56+** recommended (same floor as obs-dlss5-nr).

## Runtime file (NOT in git)

You must supply **`nvngx_dlssnr.dll`** (~158 MB, v310.8+) yourself (NVIDIA proprietary). Legitimate sources include a game that ships DLSS 5 NR (e.g. NBA 2K27) or an NVIDIA SDK package that includes the NR runtime.

Place it here:

```
%APPDATA%\CaptureView\dlss5-nr\nvngx_dlssnr.dll
```

Or set:

```powershell
$env:CAPTUREVIEW_DLSSNR_DIR = "C:\path\to\folder"   # folder that contains nvngx_dlssnr.dll
```

Also required next to that layout:

```
CAPTUREVIEW_DLSSNR_DIR\
  nvngx_dlssnr.dll
  caller\nvngx.dll_comfy.dll      # built by native/dlss5-nr/build_native.ps1
```

and `dlss5nr_bridge.dll` from `native/dlss5-nr/bin\` (auto-discovered relative to the runtime folder / repo).

The NGX core `_nvngx.dll` is loaded from the NVIDIA driver install automatically.

## Build

```powershell
# Native bridge + caller shim (MSVC required once)
powershell -ExecutionPolicy Bypass -File native\dlss5-nr\build_native.ps1

# App with NR loader
cargo run -p captureview --features dlss
cargo build --release -p captureview --features dlss
```

Default builds (no `--features dlss`) still show Mode C in the UI but report “feature disabled” and **pass through** unenhanced video (no silent NIS fake).

## Pipeline

```
capture upload (CPU→GPU)
  → color convert / deinterlace / interpolate (GPU)
  → Mode C: readback RGBA8 → D3D12 NR (feature 18) → upload → present
  → Mode A/B: FSR1 / NIS compute (GPU) → present
```

Temporal mode feeds NVIDIA Optical Flow motion vectors when the bridge enables it (same approach as obs-dlss5-nr CPU path).

## Native bridge license

`native/dlss5-nr/` is MIT-adapted from [ComfyUI-DLSS5-NR](https://github.com/lisitskyaa/ComfyUI-DLSS5-NR) (same lineage as obs-dlss5-nr’s bridge). See `native/dlss5-nr/LICENSE-ComfyUI-DLSS5-NR.txt`.

## Arthur checklist

1. RTX 50-series + recent Game Ready / Studio driver  
2. Build native bridge (`build_native.ps1`)  
3. Copy `nvngx_dlssnr.dll` into `%APPDATA%\CaptureView\dlss5-nr\` (and copy `caller\` + bridge DLL as documented)  
4. `cargo run -p captureview --features dlss`  
5. Effects → **Mode C — DLSS** — status should read Neural Rendering active  

If CreateFeature fails, the Status line shows the reason and frames pass through unchanged.
