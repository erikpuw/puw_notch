# PUW Notch

[Tiếng Việt](README.md) · **English**

A Dynamic Island–style notch for Windows. Rust + Direct2D, ~523 KB exe, no WebView.

[![Buy me a coffee](https://img.shields.io/badge/Buy%20me%20a%20coffee-PayPal-00457C?logo=paypal&logoColor=white)](https://paypal.me/erikpuw)

## Features

- **Notch**: clock, battery ring, music waves. Hover to open the panel. Shrinks when the tab bar is full of text, hides during fullscreen games and videos.
- **4 pages** (switch with ‹ ›):
  - **Music**: cover art, synced lyrics (LRCLIB), ⏮ ⏯ ⏭, click to seek. Works with any app using Windows media controls, plus iTunes and foobar2000.
  - **Tasks**: one list for tasks and appointments. Type "Buy milk", "Call 20:00" (time only = today) or "Dentist 28/10 14:30", "Call mom tomorrow 9h" (with a date = scheduled). Tick / edit / delete; overdue tasks stay, marked red. ⚙ picks an early reminder of 0–60 min, plus one on time.
  - **Notifications**: read back the ~30 latest Windows notifications (newest first, scroll with the wheel).
  - **Health**: water and stand-up reminders with adjustable times and amounts.
- **Banners**: Windows notifications, volume, brightness, mic, Caps/Num Lock, Wi-Fi, Bluetooth, network, battery.
- **Idle robot**: after 30 s without input, two LED eyes with scan lines replace the clock, look around below and blink now and then; after 5 min they fall asleep with a drifting z; when you are back they glance at the cursor, then hand the pill back to clock and battery. Toggle in the tray.
- **Tray**: Minimal notch, Idle robot, Start with Windows, Language (Tiếng Việt / English), Quit.

## Build

```bash
cargo build --release
target/release/puw_notch.exe
```

Tests: `cargo test`. Settings live in `%APPDATA%\PuwNotch\`.

Notifications are read from the Windows notification centre, so **Windows notifications must be on**. To see them only on the notch, turn off "Show notification banners" per app.

## Support

If PUW Notch helps you, buy me a coffee ☕ [paypal.me/erikpuw](https://paypal.me/erikpuw)
