# PUW Notch

[Tiếng Việt](README.md) · **English**

A Dynamic Island–style notch for Windows. Rust + Direct2D, ~523 KB exe, no WebView.

[![Buy me a coffee](https://img.shields.io/badge/Buy%20me%20a%20coffee-PayPal-00457C?logo=paypal&logoColor=white)](https://paypal.me/erikpuw)

## Features

- **Notch**: clock, battery ring, music waves. Hover to open the panel. Shrinks to a line when the pill (short or long, depending on music) would cover tabs or text underneath, and comes back when it is clear, hides during fullscreen games and videos.
- **4 pages** (switch with ‹ ›):
  - **Music**: cover art, synced lyrics (LRCLIB), ⏮ ⏯ ⏭, click to seek, the Music page fades into the cover's colour towards the bottom while playing. Works with any app using Windows media controls, plus iTunes and foobar2000.
  - **Tasks**: one list for tasks and appointments. Type "Buy milk", "Call 20:00" (time only = today) or "Dentist 28/10 14:30", "Call mom tomorrow 9h" (with a date = scheduled). Tick / edit / delete; overdue tasks stay, marked red. ⚙ picks an early reminder of 0–60 min, plus one on time.
  - **Notifications**: read back the ~30 latest Windows notifications (newest first, scroll with the wheel), each with its app's icon.
  - **Health**: water and stand-up reminders with adjustable times and amounts.
- **Banners**: Windows notifications, volume, brightness, mic, Caps/Num Lock, Wi-Fi, Bluetooth, network, battery, battery saver, USB devices plugged in and pulled out (with the device name). Line icons, each moving in its own way.
- **Idle robot**: after 30 s without input, an LED face (eyes and mouth) replaces the clock, looks around below, blinks and changes expression (curious, wink, excited; often happy while charging; sad on a low battery); after 5 min it falls asleep with a drifting pixel z; when you are back it grins at the cursor, centres itself, then hands the pill back to clock and battery. Toggle in the tray.
- **Tray**: Minimal notch, Idle robot, Start with Windows, Language (Tiếng Việt / English), Quit.

## Screenshots

| | |
|---|---|
| ![Music](assets/screenshots/page-music.png)<br>**Music**: cover, title, lyrics, background fading into the cover's colour | ![Music, nothing playing](assets/screenshots/page-music-empty.png)<br>**Music** with nothing playing |
| ![Tasks](assets/screenshots/page-tasks.png)<br>**Tasks**: tasks and appointments in one list | ![Notifications](assets/screenshots/page-notifications.png)<br>**Notifications**: each with its app's icon |
| ![Health](assets/screenshots/page-health.png)<br>**Health**: water and stand-up reminders | ![Banner](assets/screenshots/banner.png)<br>Notification **banner** |

The collapsed pill:

| Music playing | Normal | Idle robot | Happy robot |
|---|---|---|---|
| ![](assets/screenshots/pill-music.png) | ![](assets/screenshots/pill-idle.png) | ![](assets/screenshots/robot.png) | ![](assets/screenshots/robot-happy.png) |

## Build

```bash
cargo build --release
target/release/puw_notch.exe
```

Tests: `cargo test`. Settings live in `%APPDATA%\PuwNotch\`.

Notifications are read from the Windows notification centre, so **Windows notifications must be on**. To see them only on the notch, with no banner in the screen corner, turn on Windows **Do not disturb** (Settings → System → Notifications). Windows still keeps the notifications in the notification centre, so the notch reads and shows them as usual. Windows does not let another app switch off per-app banners while running, so this is yours to turn on.

## Support

If PUW Notch helps you, buy me a coffee ☕ [paypal.me/erikpuw](https://paypal.me/erikpuw)
