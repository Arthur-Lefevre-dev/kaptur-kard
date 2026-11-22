# DLSS 5 Neural Rendering + optional DLSS-G (Mode C / NVIDIA FG)

CaptureView keeps a full **NVIDIA RTX path** for future RTX users even when the developer machine is AMD. On AMD, Mode C / DLSS-G **refuse cleanly**; FSR1, NIS, and optical frame interpolate stay available.

Architecture ideas come from **public** NeuralScreen docs (`TECHNICAL.md`) — we do **not** copy PolyForm-restricted NeuralScreen source and we do **not** ship leaked / bundled NVIDIA DLLs.

## Pipeline (RTX)

```
MF capture → wgpu upload
  → color / Bob / optical interpolate (GPU)
  → Mode C: DLSS 5 NR (NGX feature 18, nvngx_dlssnr.dll)
       optional Boost: half-res NR + composite onto native frame
  → optional DLSS-G FG (nvngx_dlssg.dll, ×2–×6) — probe + UI today;
       EvaluateFeature + dedicated present queue = pending DXGI hook
  → present
```

## BYO runtimes (required)

| File | Role |
|------|------|
| `nvngx_dlssnr.dll` | DLSS 5 Neural Rendering (feature 18) |
| `nvngx_dlssg.dll` | DLSS Frame Generation (optional) |

Obtain legally (NVIDIA SDK redistributable / game that ships the public FG DLL). Place under:

```
%APPDATA%\CaptureView\dlss5-nr\
  nvngx_dlssnr.dll
  nvngx_dlssg.dll          # optional
  caller\nvngx.dll_comfy.dll
```

Or set `CAPTUREVIEW_DLSSNR_DIR` to that folder. Also need `dlss5nr_bridge.dll` from `native/dlss5-nr/build_native.ps1`.

`_nvngx.dll` is loaded from the NVIDIA driver install.

## GPU requirements

| Feature | Official / practical |
|---------|----------------------|
| NR (feature 18) | RTX **50-series** listed by NVIDIA; older may FeatureNotSupported |
| DLSS-G FG | NVIDIA floor (Ada+ for multi-frame in many runtimes); refuse below |
| Driver | 616.56+ recommended |
| API | **DX12 preferred** when built with `--features dlss` |

## Build

```powershell
powershell -ExecutionPolicy Bypass -File native\dlss5-nr\build_native.ps1
cargo run -p captureview --features dlss
```

Without `--features dlss`, Mode C / FG UI still appear but report “feature disabled” (AMD-safe default builds).

## AMD (Arthur today)

- Mode A FSR 1 / Mode B NIS — unchanged  
- Optical interpolate x2 / Auto — unchanged  
- Mode C + DLSS-G show **NVIDIA only** status — no crash, no silent fake  

## Legal

- MIT bridge under `native/dlss5-nr/` (ComfyUI-DLSS5-NR lineage)  
- No NeuralScreen source copied  
- No vendored `nvngx_*.dll`  
