# PUW Notch

[Tiếng Việt](README.md) · **English**

A Dynamic Island–style notch for Windows. Rust + Direct2D, ~523 KB exe, no WebView.

[![Buy me a coffee](https://img.shields.io/badge/Buy%20me%20a%20coffee-PayPal-00457C?logo=paypal&logoColor=white)](https://paypal.me/erikpuw)

## Features

- **Notch**: clock, battery ring, music waves. Hover to open the panel. Shrinks when the tab bar is full of text, hides during fullscreen games and videos.
- **4 pages** (switch with ‹ ›):
  - **Music**: cover art, synced lyrics (LRCLIB), ⏮ ⏯ ⏭, click to seek. Works with any app using Windows media controls, plus iTunes and foobar2000.
  - **To-do**: add / tick / edit / delete, 24h reminders ("Team call at 20:00").
  - **Appointments**: "Dentist 28/10 14:30", reminds 15 min before and on time.
  - **Health**: water and stand-up reminders with adjustable times and amounts.
- **Banners**: Windows notifications, volume, brightness, mic, Caps/Num Lock, Wi-Fi, Bluetooth, network, battery.
- **Tray**: Minimal notch, Start with Windows, Language (Tiếng Việt / English), Quit.

## Build

```bash
cargo build --release
target/release/puw_notch.exe
```

Tests: `cargo test`. Settings live in `%APPDATA%\PuwNotch\`.

Notifications are read from the Windows notification centre, so **Windows notifications must be on**. To see them only on the notch, turn off "Show notification banners" per app.

## Support

If PUW Notch helps you, buy me a coffee ☕ [paypal.me/erikpuw](https://paypal.me/erikpuw)
