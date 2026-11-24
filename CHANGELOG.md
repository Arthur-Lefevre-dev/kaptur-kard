# Changelog

## v0.6.0

### KAP-66 — Audio HUD & polish
- **F4** toggles a per-channel audio visualization HUD (L/R + C/Ls/Rs/LFE when present) with VU bars, peak hold, and short level graphs. Sound-tab meters stay in sync.
- Removed the compact latency sparkline under the FPS / 1% low / Latency HUD text (numeric latency remains).
- Startup hotkeys tips and update-available box no longer overlap — stacked/offset so both stay readable.
- Update modal shows **patch notes** from the GitHub release body (Notes de version).

### KAP-67 — Keyboard rebind
- New **Raccourcis / Keys** settings tab (after Clips) to rebind F1–F4 overlays, clip save, fullscreen, mute, etc.
- Click-to-rebind with conflict swap, per-key reset, and reset-all. Persisted in `ui.toml`.
