# CaptureView

Desktop preview app for generic UVC / capture-card sources. Product name **CaptureView**; workspace/binary crate **`captureview`**; repository folder **`kaptur-kard`**.

## Status

**Step (d)** — consumer UI + i18n (FR/EN).

- Full-frame video with floating toolbar (Image / Son / Effets / Infos / Plein écran) on mouse move; hides after 3s
- Settings panel with 4 tabs; technical options under **Avancé**
- First-run wizard (≤3 steps, skippable)
- Hot changes auto-saved; **Tout remettre en automatique**
- Human error messages + **Copier le rapport pour le support**
- i18n via `sys-locale` + `locales/fr.toml` / `locales/en.toml`
- Shortcuts: F1 menu, F2 infos, F11 fullscreen, M mute
- Effets: FSR / NIS / interpolation greyed “Bientôt”

## Build & run

```powershell
cd c:\Users\kingd\Documents\GitHub\kaptur-kard
cargo run -p captureview
cargo build --release -p captureview
cargo test --workspace
```

Ideal path: launch → card selected automatically (Auto) → picture fills the window. Move the mouse for the toolbar.

## Out of scope (later)

- Real FSR / NIS / interpolation shaders
- Detailed metrics graphs (step e)
- Linux / macOS

## Licence

MIT — see [LICENSE](./LICENSE).
