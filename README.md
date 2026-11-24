<p align="center">
  <img src="assets/bcb.png" width="132" alt="BetterClipboard icon">
</p>

<h1 align="center">BetterClipboard</h1>

<p align="center">
  <b>The Windows 11 clipboard history, but faster and with the parts that are missing.</b><br>
  Screenshots you can save, entries from your phone, search, and a 1&nbsp;MB native Rust app that opens in about 20&nbsp;ms.
</p>

<p align="center">
  <a href="https://github.com/TabanAziz/BetterClipboard/releases/latest"><b>Download</b></a> ·
  <a href="#install">Install</a> ·
  <a href="#what-you-get">Features</a> ·
  <a href="#the-numbers">Numbers</a> ·
  <a href="#build-from-source">Build</a> ·
  <a href="docs/limitations.md">Limitations</a>
</p>

<p align="center">
  <img src="assets/screenshots/panel-dark.png" width="360" alt="Clipboard panel with text, a phone entry and a screenshot">
  &nbsp;
  <img src="assets/screenshots/panel-light-off.png" width="360" alt="Clipboard panel in light mode with history turned off">
</p>

Part of the Better series, next to [Better Task Manager](https://github.com/TabanAziz/BetterTaskManager).

## Why

Win+V is handy until you need it for more than the last line of text. A screenshot you snipped ten minutes ago is in there, but you cannot save it. Something you copied on your phone shows up, but nothing tells you which entry it was. There is no search, and Windows documents a limit of 4 MB per entry, so big pictures can drop out of the history without a word.

BetterClipboard keeps the panel people already know, the same layout and the same click paths, and adds what is missing.

## What you get

| | |
| --- | --- |
| **Familiar panel** | Grip, tab row, "Clipboard" with "Clear all", entry cards with "See more" (...) for pin and delete, "Let's get started / Turn on" when history is off. Light and dark like your Windows theme. |
| **Paste where you were** | Pick an entry with the mouse or arrows and Enter: it goes into the app you came from. Shift+Enter pastes plain text, Ctrl+Enter only copies. |
| **Pictures** | Previews for screenshots and copied images, "Save image as" (PNG or BMP, Ctrl+S) and "Open image". Full-screen 4K screenshots are kept. |
| **Your phone** | With Phone Link's cross-device copy and paste, what you copy on your Android phone lands on the PC clipboard. BetterClipboard marks those entries with a phone icon ("From your phone") and has a Phone filter. |
| **Emoji and symbols** | Emoji (in colour, like Windows), kaomoji and symbol tabs inside the panel: pick a character and it is typed into the app you came from, without touching your clipboard. Recently used ones sit under the heart. |
| **Find things** | Type to search, filters for text, images, files, phone and pinned. Identical copies are grouped. |
| **More than text** | Text, links, HTML and RTF keep their formatting; files copied in Explorer can be pasted again. |
| **Keep what matters** | Pinned entries survive restarts, encrypted with DPAPI for your Windows account. Optional: keep the whole history, longer history than 25 entries, automatic removal after N days, snippets you type yourself. |
| **Privacy** | Copies that password managers mark as private are never recorded. Pause recording from the tray, exclude apps, save nothing to disk at all if you want. No network, no telemetry. |

<p align="center">
  <img src="assets/screenshots/phone-filter.png" width="270" alt="Phone filter with a text and a photo from the phone">
  <img src="assets/screenshots/see-more.png" width="270" alt="See more reveals pin and delete">
  <img src="assets/screenshots/emoji.png" width="270" alt="Emoji page">
</p>

<p align="center">
  <img src="assets/screenshots/settings.png" width="560" alt="Settings in the style of Windows 11 Settings">
</p>

## The numbers

| BetterClipboard 0.1.3 | |
| --- | --- |
| Shortcut until the painted panel, 25 entries | **13 ms** median, 21 ms p95 |
| Same with 1 000 entries | **13 ms** median, 20 ms p95 |
| Keystroke in search until painted, 10 000 entries | **13 ms** median, 14 ms p95 |
| Emoji page repaint | **about 1 ms** |
| Start until ready (warm) | **26 ms** median |
| CPU while idle | **0 ms** in 5 minutes |
| Size of the program | **1.1 MB**, no runtime, no administrator rights |

<sub>Measured on an Intel Core i7-11800H laptop with Windows 11 25H2 at 100% scaling. Panel times include waiting for the compositor to show the frame. Method, raw data and idle CPU in <a href="docs/benchmarks.md">docs/benchmarks.md</a>. Your numbers will differ.</sub>

## Install

1. Download **`BetterClipboard-Setup-0.1.3.exe`** from the [latest release](https://github.com/TabanAziz/BetterClipboard/releases/latest).
2. Run it and pick how your clipboard history should open:
   - **With Win+V (recommended):** BetterClipboard replaces the Windows clipboard panel. Explorer restarts once so it works right away.
   - **Keep Win+V for Windows:** BetterClipboard opens with Win+Alt+V.
3. Click Install, then press Win+V.

<p align="center">
  <img src="assets/screenshots/setup.png" width="520" alt="Setup dialog">
</p>

Setup installs for your account only (`%LOCALAPPDATA%\Programs\BetterClipboard`), adds a Start menu entry and an "Installed apps" entry, and optionally starts BetterClipboard when you sign in. No administrator rights are needed. The program is not code-signed yet, so Windows SmartScreen may ask once: choose "More info" and "Run anyway".

Uninstall from Settings > Apps > Installed apps. That also gives Win+V back to Windows.

Prefer no installer? The release also has a portable zip; run `BetterClipboard.exe` from anywhere. For scripted installs: `BetterClipboard.exe --install --use-win-v --quiet`.

## Use

- **Win+V** (if you chose it in setup) or **Win+Alt+V** opens the history. Clicking the tray icon works too.
- Switch between the two any time in Settings > "Open with Win+V", or with `--use-win-v` / `--windows-win-v`. Explorer restarts once, no sign-out needed.
- Right-click an entry or press the Menu key for everything else: paste as plain text, copy only, pin, delete, save or open an image, show a file in Explorer.
- To get your phone's clipboard: in Phone Link, turn on "Cross-device copy and paste" (Settings > Features). It works with supported Android phones.

Command line: `--show`, `--settings`, `--snippet`, `--quit`, `--background`, `--setup`, `--install`, `--uninstall`, `--use-win-v`, `--windows-win-v`.

## System requirements

Windows 11 (built and tested on 25H2, build 26200), x64. Windows 10 should work but is not tested.

## Build from source

Rust stable with the MSVC toolchain:

    cargo build --release
    powershell -File tools\check.ps1

The program ends up in `target\release\BetterClipboard.exe`. `tools\release.ps1` builds the setup file, the portable zip and checksums in `dist\`. The setup file is the same program under a name containing "Setup", which makes it show the setup dialog. Tests that use the real clipboard: see [CONTRIBUTING.md](CONTRIBUTING.md).

## Known limitations

The short version: taking over Win+V switches off the shortcut of the Windows panel (Explorer restarts once), Windows' own history and its cloud sync are not readable by other apps, apps running as administrator cannot be pasted into, there is no GIF search, and screen reader support in the panel is basic. Phone entries are recognised by the Phone Link process and were tested with a simulated sender, not a real phone. Everything is listed in [docs/limitations.md](docs/limitations.md), and the feature-by-feature comparison with Windows is in [docs/parity-matrix.csv](docs/parity-matrix.csv).

## More

- [Architecture](docs/architecture.md)
- [Test plan and results](docs/test-plan.md)
- [Benchmarks](docs/benchmarks.md)

## License

MIT, see [LICENSE](LICENSE). Third-party crates are listed in [THIRD-PARTY-LICENSES](THIRD-PARTY-LICENSES).
