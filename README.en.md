# PUW Notch

[Tiếng Việt](README.md) · **English**

A Dynamic Island–style notch for Windows. Rust + Direct2D, ~523 KB exe, no WebView.

## Features

- **Notch**: clock, battery ring, music waves. Hover to open the panel. Shrinks to a line when the pill (short or long, depending on music) would cover tabs or text underneath, and comes back when it is clear, hides during fullscreen games and videos.
- **4 pages** (switch with ‹ ›):
  - **Music**: cover art, synced lyrics (LRCLIB), ⏮ ⏯ ⏭, click to seek, a lyrics button widens the panel sideways to show scrolling lyrics (the line being sung lit in the middle, long lines wrap), the Music page fades into the cover's colour towards the bottom, with a thin glowing aurora line in the cover's two strongest colours swaying slowly over it while playing (sharp core, halo drawn small and stretched for the blur, so it stays light). Works with any app using Windows media controls, plus iTunes and foobar2000.
  - **Tasks**: one list for tasks and appointments. Type "Buy milk", "Call 20:00" (time only = today) or "Dentist 28/10 14:30", "Call mom tomorrow 9h" (with a date = scheduled). Tick / edit / delete; overdue tasks stay, marked red. ⚙ picks an early reminder of 0–60 min, plus one on time.
  - **Notifications**: read back the ~30 latest Windows notifications (newest first, scroll with the wheel), each with its app's icon. Click a row to open the app that sent it (or the notification's link / picture), as in Windows' notification centre.
  - **Health**: water and stand-up reminders with adjustable times and amounts.
- **Banners**: Windows notifications, volume, brightness, mic, Caps/Num Lock, Wi-Fi, Bluetooth, network, battery, battery saver, USB devices plugged in and pulled out (with the device name). Line icons, each moving in its own way. Take a screenshot with Snipping Tool (Win+Shift+S) and the banner shows a thumbnail; click it to open the shot. Click a notification banner to open the app that sent it (or the notification's link, e.g. Windows Security); it opens the app, not the exact message.
- **Idle robot**: after 30 s without input, an LED face (eyes and mouth) replaces the clock, looks around below, blinks and changes expression (curious, wink, excited; often happy while charging; sad on a low battery); after 5 min it falls asleep with a drifting pixel z; when you are back it grins at the cursor, centres itself, then hands the pill back to clock and battery. Toggle in the tray.
- **Hide Windows volume popup** (tray, off by default): the app handles the volume up / down / mute keys itself, so only the notch shows and Windows' OSD stays away. Laptop brightness and mic keys usually go through firmware and are not blocked.
- **Weather** (5th page of the panel, opened from the pill): type a city once, with or without Vietnamese accents ("Đà Nẵng", "Da Nang", "TP. HCM", "Saigon"); the app strips the accents, looks up the coordinates and remembers them in `settings.txt`. The pill shows an icon and the temperature ("☁ 28°") beside the clock, plain or with music; the Weather page has a large icon (sun, moon, partly cloudy, cloudy, fog, rain, snow, thunderstorm), the sky in words, feels-like temperature, humidity and wind. Temperatures are in °C or °F: by default what Windows' region setting says (US measurement system means °F); click the °C | °F buttons beside the temperature to switch, and the app remembers it. Refreshed every 30 minutes on a worker thread (and right away when the network is back or the PC wakes), retried after 1 minute on failure. Data from [Open-Meteo](https://open-meteo.com) (CC BY 4.0, free, no key), icons from Tabler Icons.
- **Tray**: Minimal notch, Idle robot, Hide Windows volume popup, Start with Windows, Language (Tiếng Việt / English), About PUW Notch (version, description, GitHub link), Quit.

## Screenshots

| | |
|---|---|
| ![Music](assets/screenshots/page-music.png)<br>**Music**: cover, title, lyrics, background and aurora line in the cover's colours | ![Music, nothing playing](assets/screenshots/page-music-empty.png)<br>**Music** with nothing playing |
| ![Tasks](assets/screenshots/page-tasks.png)<br>**Tasks**: tasks and appointments in one list | ![Notifications](assets/screenshots/page-notifications.png)<br>**Notifications**: each with its app's icon |
| ![Health](assets/screenshots/page-health.png)<br>**Health**: water and stand-up reminders | ![Banner](assets/screenshots/banner.png)<br>Notification **banner** |

![Music with lyrics](assets/screenshots/page-music-lyrics.png)<br>**Music** with lyrics: the panel widens, the line being sung lit in the middle, the aurora line swaying along the bottom

The collapsed pill:

| Music playing | Normal | Idle robot | Happy robot |
|---|---|---|---|
| ![](assets/screenshots/pill-music.png) | ![](assets/screenshots/pill-idle.png) | ![](assets/screenshots/robot.png) | ![](assets/screenshots/robot-happy.png) |

## Download and run

Get `puw_notch.exe` from [Releases](https://github.com/erikpuw/puw_notch/releases) and run it. The app is not code-signed yet, so Windows may block it:

- **SmartScreen** ("Windows protected your PC"): click **More info** → **Run anyway**.
- **File marked as downloaded**: right-click the file → **Properties** → tick **Unblock** → OK.
- **Smart App Control** (Windows 11, shows "App blocked" with no way to continue): turn it off in Settings → Privacy & security → Windows Security → App & browser control → Smart App Control settings → **Off**. Once off it cannot be turned back on without reinstalling Windows, so think first. The alternative is to build from source (next section); a build made on your own machine usually runs.

## Build

```bash
cargo build --release
target/release/puw_notch.exe
```

Tests: `cargo test`. Settings live in `%APPDATA%\PuwNotch\`. Starts, exits, failures and self-hiding are logged to `%APPDATA%\PuwNotch\notch.log` (64 KB at most), to tell why a start with Windows did not come up.

Notifications are read from the Windows notification centre, so **Windows notifications must be on**. To see them only on the notch, with no banner in the screen corner, turn on Windows **Do not disturb** (Settings → System → Notifications). Windows still keeps the notifications in the notification centre, so the notch reads and shows them as usual. Windows does not let another app switch off per-app banners while running, so this is yours to turn on.

## Support

If PUW Notch helps you, buy me a coffee ☕ [paypal.me/erikpuw](https://paypal.me/erikpuw)
