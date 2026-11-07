<p align="center">
  <img src="assets/puw_1024.png" width="112" alt="PUW Notch icon">
</p>

<h1 align="center">PUW Notch</h1>

<p align="center"><b>A notch that lives on your Windows.</b></p>

<p align="center">
  Music, tasks, notifications, weather, a calendar and a tiny robot, tucked into the top edge of your screen, Dynamic Island style.<br>
  Rust + Direct2D drawn on the GPU through DirectComposition, ~794 KB exe, no WebView.
</p>

<p align="center"><a href="README.md">Tiếng Việt</a> · <b>English</b></p>

> **Download only from the author's source.** PUW Notch is published by its author (erikpuw) only at [github.com/erikpuw/puw_notch](https://github.com/erikpuw/puw_notch). A copy from anywhere else (file-sharing sites, other repositories, repackaged builds, files sent over the internet) is not controlled by the author and comes with no guarantee: it may have been modified or had code added. Use only the build in this repository's [Releases](https://github.com/erikpuw/puw_notch/releases).

## Features

| Feature | What it does |
|---|---|
| **Notch** | Clock, battery ring (green while charging, yellow in battery saver, red at 20 %) and music waves. Hover to open the panel. Two looks, chosen in the tray: ears or a floating pill. |
| **Stays clear** | Shrinks when it would cover text underneath (tabs in Edge or Terminal), hides during fullscreen games and videos. It asks the window beneath it instead of capturing the screen, so it still shows in recordings, screen shares and remote desktop. |
| **Banners** | Windows notifications (the app's icon, the time received, an × to close early, a thumbnail for screenshots), volume, brightness, mic, Caps/Num Lock, battery, battery saver, USB, Wi-Fi, Bluetooth, airplane mode. Notifications that arrive together show one after another, in the order they came. |
| **Network banners** | Wi-Fi, Bluetooth and airplane mode each have their own banner; airplane mode gathers what goes with it. When Wi-Fi comes on or airplane mode goes off, the banner is a list of steps (the network connection, the VPN that was up before, Bluetooth and the devices that were connected): a green tick when done, a spinner while waited for, an empty ring when not yet its turn, a red cross after 15 s without success. One Bluetooth device is its own banner with the battery large; two or more are a list with small batteries. Switching off and on quickly leaves only the last state. |
| **Live tasks** | Progress banners for copying, moving and deleting files in Explorer, downloads and terminal builds. Off by default, [details](#live-tasks). |
| **Approve on the notch** | Questions from Claude Code, Codex and Antigravity (permission to use a tool, multiple-choice questions) show as banners; click **Allow** / **Deny** or pick an answer. No auto-approve. The banner closes after 20 s (the app asks as usual) or as soon as you answer in the app. Setup [below](#setting-up-claude-code-optional). |
| **Robot** | After 30 s without input an LED face (just a pair of eyes) replaces the clock: it looks around, cheers when the battery has just filled up, and is sad at 20 % or less while not charging. Move the mouse and it glances at the cursor; after 5 min it sleeps. Toggle in the tray. |
| **Light** | Loads only what is visible and pauses while hidden for fullscreen. About 7 MB of RAM and 0 % CPU at rest, about 14 MB while music plays in the pill, about 22 MB with the Music page open and its aurora ([Task Manager pictures](#light-and-the-tray-menu)). |
| **Tray** | Minimal notch, Idle robot, Notch style, Pages shown, More features (Approve on the notch, Live tasks, Hide Windows volume popup), Start with Windows, Language (Tiếng Việt / English), About, Quit. |

A page hidden in the tray stops completely: no task, water or stand-up reminders, no Pomodoro, and no music, weather, limits or notifications are read.

**Start with Windows** asks for administrator permission once to create a `PuwNotch` task in Task Scheduler. From then on the notch starts with administrator rights 15 s after you sign in, with no prompt, and can read administrator windows too. Untick it and the task is removed.

**8 pages**, switched with ‹ › or by putting the mouse on the page dots: they open into a row of icons to pick a page at once.

| Page | What it does |
|---|---|
| **Music** | Cover, synced lyrics (LRCLIB), ⏮ ⏯ ⏭, shuffle, repeat, drag the bar to seek (Windows media controls, iTunes, foobar2000; a button the player lacks stays dim). The lyrics button widens the panel; background and aurora follow the cover. |
| **Notes** | Notes and appointments in one list, a progress ring says how many are left. Type "Call 20:00", "Dentist 28/10 14:30" or "Call mom tomorrow 9h". Overdue shows red "Late"; ⚙ picks a reminder 0 to 60 min ahead. |
| **Focus** | Pomodoro: the whole cycle (4 focus rounds of 25 min, short breaks of 5, a long one of 15) is one segmented strip. When a segment ends a banner says so and the next one starts by itself (after the long break it waits for a new cycle). While it runs the pill has a thin line along its bottom. |
| **Notifications** | The ~30 latest Windows notifications, each with its app icon and two lines. Click to open the app; "Clear all" only hides them in the notch, Windows keeps them. |
| **Health** | Water and stand-up reminders: the day's progress as a bar, a round button to log a drink or having stood up, times and amounts under ⚙. |
| **Weather** | Type a city once: a large temperature, the next 5 slots, feels-like, humidity, wind, the day's high, °C \| °F. The sun button swaps the hours for **the sun's path** (sunrise, sunset, daylight left; a crescent moon at night). Open-Meteo data, refreshed every 30 min. |
| **Calendar** | One month, weeks from Monday to Sunday (ISO 8601), the day picked shown large on the left with its weekday. Click a day to look at it, ‹ › change the month. No holidays, since each country has its own. On a PC set to Vietnam, China, Hong Kong, Macao, Taiwan or Korea, a **Solar \| Lunar** choice swaps the grid's numbers for the lunar calendar (worked out in the app, no network). Change the region in Windows and the choice shows or hides with it. |
| **Limits** | 5-hour and weekly limits of Claude, Codex and Antigravity (a free plan: 30 days), a tab per app, only for the apps found on the PC. Claude shows what is used (two concentric rings: 5 hours outside, the week inside); Codex and Antigravity show what is left. A banner warns at 80 %, 95 %, or 25 % of the limit in one day. |

The Limits page reads this PC only, nothing is sent:

| App | Read from |
|---|---|
| **Claude** | `plan-usage-history.json` that the desktop app writes itself (looked for in `%APPDATA%\Claude`, `Claude-Data` and the app's package folder; an old reading still shows with its age, open the usage popup in the app to refresh it). Claude Code in a terminal through `statusLine`. |
| **Codex** | Session logs in `~/.codex`, updated after each agent turn. A reading over 10 min old says "Old"; after switching accounts it waits for the first turn. |
| **Antigravity** | The app's own local server, only while it is open. |

Credits: weather data [Open-Meteo](https://open-meteo.com) (CC BY 4.0), weather and battery icons [Lucide](https://lucide.dev) (ISC), banner icons [Tabler Icons](https://tabler.io/icons) (MIT).

## Live tasks

Turn on in **tray → More features → Live tasks** (off by default). One task shows at a time, Explorer first, then downloads, then builds; with more running, the title gets a **+N**.

- **Explorer:** copying, moving, deleting and extracting, with the percent and a progress bar. It cannot catch an operation that is too quick or has no progress window.
- **Downloads:** the Downloads folder Windows is set to and the download folders of Edge, Chrome and Brave. Shows the file name, MB downloaded and MB/s; when it finishes, click the banner to open the folder. No percent, and Firefox is not supported yet.
- **Builds:** cargo, msbuild, dotnet, make, cmake, ninja and go run from a terminal. The end shows a green tick or a red ✕ by the exit code.

## Screenshots

| Screenshot | Screenshot |
|---|---|
| ![Music](assets/screenshots/en/page-music.png)<br>**Music**: cover, title, lyrics, background and aurora line in the cover's colours | ![Music, nothing playing](assets/screenshots/en/page-music-empty.png)<br>**Music** with nothing playing |
| ![Notes](assets/screenshots/en/page-tasks.png)<br>**Notes**: notes and appointments in one list | ![Notifications](assets/screenshots/en/page-notifications.png)<br>**Notifications**: each with its app's icon |
| ![Focus](assets/screenshots/en/page-focus.png)<br>**Focus**: the whole Pomodoro cycle on one strip | ![Pill while counting](assets/screenshots/pill-focus.png)<br>**Pill** while it counts: a thin line along its bottom |
| ![Health](assets/screenshots/en/page-health.png)<br>**Health**: water and stand-up reminders | ![Banner](assets/screenshots/en/banner.png)<br>Notification **banner** |

![Music with lyrics](assets/screenshots/en/page-music-lyrics.png)<br>**Music** with lyrics: the panel widens, the line being sung lit in the middle, the aurora line swaying along the bottom

![Quick page picker](assets/screenshots/en/page-nav.png)<br>**Quick page picker**: put the mouse on the page dots and they open into a row of icons, click one to jump straight to that page

| ![Weather](assets/screenshots/en/page-weather.png)<br>**Weather**: large icon, temperature, the next 5 slots, °C / °F buttons | ![The sun's path by day](assets/screenshots/en/page-weather-sun.png)<br>**By day**: a blue arc from sunrise to sunset, the sun standing at the time now | ![The sun's path by night](assets/screenshots/en/page-weather-night.png)<br>**By night**: a crescent moon walks the arc from sunset to sunrise |
|---|---|---|

| ![Calendar](assets/screenshots/en/page-calendar.png)<br>**Calendar**: the day large, weeks from Monday, ‹ › change the month | ![Lunar calendar](assets/screenshots/en/page-calendar-lunar.png)<br>**Lunar**: the Solar \| Lunar choice swaps the grid's numbers (only where the region has one) |
|---|---|

| ![Wi-Fi on](assets/screenshots/en/banner-wifi.png)<br>**Wi-Fi on**: the connection, then the VPN that was up before | ![Airplane mode off](assets/screenshots/en/banner-airplane-off.png)<br>**Airplane mode off**: the steps are listed from the start and tick off as they happen |
|---|---|
| ![Airplane mode on](assets/screenshots/en/banner-airplane-on.png)<br>**Airplane mode on**: Wi-Fi, Bluetooth and the VPN go off with it | ![Two Bluetooth devices](assets/screenshots/en/banner-bluetooth.png)<br>**Bluetooth**: two or more devices, a line each with its battery |

**Limits** of Claude, Codex and Antigravity. The Codex tab names the plan of the account in use; a free plan counts 30 days:

| Claude | Codex (free plan, 30 days) | Antigravity |
|---|---|---|
| ![](assets/screenshots/en/page-usage-claude.png) | ![](assets/screenshots/en/page-usage-codex.png) | ![](assets/screenshots/en/page-usage-antigravity.png) |

**Water and stand-up reminders**: the banner only informs (× closes it). To log a drink or that you stood up, press the round button on the Health page ("Later, 10 min" is there too):

| Time to drink | Goal reached | Sitting too long |
|---|---|---|
| ![](assets/screenshots/en/banner-water.png) | ![](assets/screenshots/en/banner-water-goal.png) | ![](assets/screenshots/en/banner-standup.png) |

**Approve on the notch**: a click answers.

| Claude (a multiple-choice question) | Antigravity (permission to use a tool) |
|---|---|
| ![](assets/screenshots/en/banner-approve-claude.png) | ![](assets/screenshots/en/banner-approve-antigravity.png) |

**Pill style** (tray, Notch style): floats below the top edge of the screen with all four corners round. The ears remain the default.

| Resting | Playing music | Open page |
|---|---|---|
| ![](assets/screenshots/style-pill-idle.png) | ![](assets/screenshots/style-pill-music.png) | ![](assets/screenshots/en/style-pill-page.png) |

The collapsed pill:

| Music playing | Normal | Idle robot | Happy robot |
|---|---|---|---|
| ![](assets/screenshots/pill-music.png) | ![](assets/screenshots/pill-idle.png) | ![](assets/screenshots/robot.png) | ![](assets/screenshots/robot-happy.png) |

With the weather on:

| Normal | Music playing |
|---|---|
| ![](assets/screenshots/weather-pill.png) | ![](assets/screenshots/weather-pill-music.png) |

## Light and the tray menu

Task Manager in three states (measured on one PC, the figures vary by PC):

| The notch at rest | Music playing, as the pill | The Music page open, with the aurora |
|---|---|---|
| ![](assets/screenshots/task-manager.png)<br>0 % CPU, 7.3 MB of RAM | ![](assets/screenshots/task-manager-music-pill.png)<br>0.4 % CPU, 14.5 MB of RAM | ![](assets/screenshots/task-manager-music-page.png)<br>0.5 % CPU, 21.6 MB of RAM |

The tray menu (right-click the icon):

| Tiếng Việt | English |
|---|---|
| ![](assets/screenshots/tray-vi.png) | ![](assets/screenshots/tray-en.png) |

## Download and run

Get `puw_notch.exe` from [Releases](https://github.com/erikpuw/puw_notch/releases) and run it (download only from here, see the note at the top). The app is not code-signed, so nothing but this source confirms a file is the author's, and Windows may block it:

- **SmartScreen** ("Windows protected your PC"): click **More info** → **Run anyway**.
- **File marked as downloaded**: right-click the file → **Properties** → tick **Unblock** → OK.
- **Smart App Control** (Windows 11, shows "App blocked" with no way to continue): turn it off in Settings → Privacy & security → Windows Security → App & browser control → Smart App Control settings → **Off**. Once off it cannot be turned back on without reinstalling Windows, so think first. The alternative is to build from source (next section); a build made on your own machine usually runs.

## Build

```bash
cargo build --release
target/release/puw_notch.exe
```

The release build uses `opt-level = "z"` (an exe of ~794 KB instead of ~810 KB, with no change in the time to draw a frame). `build.rs` embeds the icon and version info (taken from `Cargo.toml`) in the exe, shown under Properties → Details. Tests: `cargo test`. Settings live in `%APPDATA%\PuwNotch\`. Starts, exits, failures, self-hiding, the panel opening and closing, late frames and the GPU drawing the notch are logged to `%APPDATA%\PuwNotch\notch.log` (64 KB at most, started over when the version changes), to tell why a start with Windows did not come up.

Notifications are read from the Windows notification centre, so **Windows notifications must be on**. To see them only on the notch, with no banner in the screen corner, turn on Windows **Do not disturb** (Settings → System → Notifications). Windows still keeps the notifications in the notification centre, so the notch reads and shows them as usual. Windows does not let another app switch off per-app banners, so this is yours to turn on.

## Setting up Claude Code (optional)

Approving on the notch and the limits of Claude Code in a terminal need two small scripts from `tools/`. Copy them to `~/.claude/` and add this to `~/.claude/settings.json` (replace `<name>` with your Windows user name):

```json
"statusLine": { "type": "command", "command": "powershell -NoProfile -ExecutionPolicy Bypass -File C:/Users/<name>/.claude/puw-notch-statusline.ps1" },
"hooks": {
  "PermissionRequest": [
    { "matcher": "*", "hooks": [ { "type": "command", "command": "powershell -NoProfile -ExecutionPolicy Bypass -File C:/Users/<name>/.claude/puw-notch-approve.ps1", "timeout": 60 } ] }
  ]
}
```

- `puw-notch-approve.ps1` (hook): sends the permission request to the notch over a local pipe. If the notch is not running, nobody clicks within 50 seconds, or you press **In the app**, Claude asks as usual. There is no auto-approve.
- `puw-notch-statusline.ps1` (statusLine): only needed for Claude Code in a terminal; for the Claude desktop app the notch reads the app's own file.
- Antigravity needs no setup: the notch reads its local server (while it is open). Codex: the limits need no setup, approving commands is in the next section.

## Setting up Codex (optional)

The notch reads Codex's limits straight from `~/.codex`, no setup. To **approve Codex's commands on the notch**, share the `puw-notch-approve.ps1` script with Claude Code: copy it to `~/.claude/` (or wherever you like) and add a `PermissionRequest` entry to `~/.codex/hooks.json` (replace `<name>` with your Windows user name):

```json
{
  "hooks": {
    "PermissionRequest": [
      { "hooks": [ { "type": "command", "command": "powershell -NoProfile -ExecutionPolicy Bypass -File C:/Users/<name>/.claude/puw-notch-approve.ps1 -App Codex" } ] }
    ]
  }
}
```

- If `hooks.json` already has a `PermissionRequest` entry, add the group above at the **end** of its array so the older hooks keep their positions (Codex stores its trust hash by position).
- Codex only runs hooks it trusts: open Codex, type `/hooks` and trust the new hook.
- From then on, when Codex asks for permission, a **Codex** banner with the command shows on the notch; click **Allow** or **Deny**. If the notch is not running, or nobody clicks within 50 seconds, Codex asks as usual.
- `-App` is the name on the banner. `-Log` (added at the end of the command) writes what the hook receives to `%TEMP%\puw-notch-hook.log`, only to see what an app sends; do not leave it on, as it stores the commands too.

## Support

If PUW Notch helps you, buy me a coffee ☕ [paypal.me/erikpuw](https://paypal.me/erikpuw)
