# VRR (G-Sync / FreeSync) and HDR

CaptureView present path notes for Windows (primary). Prefer **DX12** — the app already scores DX12 above Vulkan when both adapters work.

## VRR / tearing

| Item | Behavior |
|------|----------|
| Detection | Surface `PresentMode::Immediate` (wgpu DX12 only advertises it when DXGI `ALLOW_TEARING` is supported) + optional DXGI factory probe |
| When VSync Auto/Off (NoVsync path) | Prefer **Immediate**, then **Mailbox**, then `AutoNoVsync` |
| When VSync On | **Fifo** (or `AutoVsync`) |
| Pacing | Software `capture_wake` / display FPS limit still schedules presents — VRR does not replace framegen or wake pacing |
| Fullscreen | **Borderless** fullscreen (current app path). DXGI tearing works in windowed and borderless; exclusive / true fullscreen is not used |

Mailbox is tear-free “fast vsync” (single-frame queue). Immediate allows tearing and is what VRR panels need for unrestricted refresh.

## HDR

| Item | Behavior |
|------|----------|
| Format | `Rgba16Float` swapchain → **scRGB** on DX12 (DWM default for fp16); Vulkan uses extended-sRGB linear for the same format |
| When | Settings **HDR output** On **and** Windows Advanced Color enabled on an active path |
| Fallback | Non-sRGB 8-bit (`BGRA8` / `RGBA8`) when OS HDR is off, toggle is Off, or fp16 is missing |
| Content | Capture is typically SDR; values stay in 0–1 (SDR reference white). No HDR10 PQ encode of the capture stream |
| Toggle | Effects settings; persisted in `%APPDATA%\CaptureView\ui.toml` as `hdr_output` |

Changing HDR rebuilds the surface format–dependent pipelines (video / upscale / interpolate / egui).

## DX12 vs Vulkan

- **DX12 (preferred)**: reliable Immediate + ALLOW_TEARING; fp16 ≈ scRGB via DWM without an extra color-space API in wgpu 24.
- **Vulkan**: Immediate/Mailbox depend on the driver; HDR uses `EXTENDED_SRGB_LINEAR` for `Rgba16Float`. Overlay layers can hitch — prefer DX12 on Windows.

## Metrics / UI

Infos and Effects show:

- VRR capable or not, plus the active present mode label
- HDR active / OS off / unsupported / user off, plus surface format

## Limits (wgpu 24)

- No `SurfaceColorSpace` / `display_hdr_info` (those land in newer wgpu). scRGB is implied by fp16 on DX12.
- No exclusive fullscreen VRR path.
- HDR10 PQ / BT.2020 capture→display remap is out of scope; scRGB window output only.
