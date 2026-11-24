# Test plan

Reference PC: Windows 11 Pro 25H2, build 10.0.26200.9974, de-AT region, 1920×1080 at 100%, dark app theme, Intel Core i7-11800H, 16 GB RAM. The status column records what was actually run there for version 0.1.0.

## Automated

| Suite | Command | What it covers | Result |
| --- | --- | --- | --- |
| Unit tests | `cargo test` | History limits, pinning, grouping, echo suppression, clear all, retention, search incl. 10 000 entries, Phone Link recognition, DIB decoding (1/4/8/16/24/32 bit, top-down, bottom-up, bitfields, garbage), PNG round trips, CF_HDROP and HTML Format parsing, RTF preview, settings parsing, hotkey parsing, store encode/decode, truncated and tampered files, DPAPI encryption, atomic save, damaged file kept aside | 39 passed |
| System clipboard | `cargo test --test system_clipboard -- --ignored --test-threads=1` | Real clipboard: text, HTML + RTF, CF_DIB, PNG with alpha, files with drop effect, private markers, `CanIncludeInClipboardHistory`, 4 MB limit, 4K screenshot, unknown and broken formats, paused capture, writing back all formats, plain-text mode, files, 200 copies in a row, clipboard held open by another program | 4 passed (debug and release) |
| Lint | `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` | | clean |

## End to end on the desktop

These were run with PowerShell scripts that start BetterClipboard in a separate profile (`BETTERCLIPBOARD_PROFILE`), so the user's history is never touched.

| ID | Scenario | How | Result |
| --- | --- | --- | --- |
| C01 | Copy text, Unicode, link, picture, phone picture, files from other processes; panel shows them with sources | WinForms clipboard calls from PowerShell, then the panel | Passed (screenshots) |
| C02 | Normal Ctrl+V still works | Clipboard read after BetterClipboard wrote an entry; the listener never writes on its own | Passed |
| C03 | Choose an entry and paste into the previous app | Real Win+Alt+V, Down, Enter into a RichTextBox window; text read back | Passed: `Ünïcödé ✓ 日本語 🌍 entry` arrived |
| C04 | 1 000 and 10 000 entries, 4 MB limit, broken formats, fast copying | Benchmarks and system clipboard tests | Passed |
| C05 | Pin, restart, Clear all | Pin by click, `--quit`, restart: only the pinned entry remained; file contains no plain text | Passed |
| C06 | Clipboard held open, delayed rendering | System clipboard test (held for 250 ms); delayed rendering is served by the owner inside `GetClipboardData` on the clipboard thread | Passed (lock); delayed rendering not separately simulated |
| C07 | Administrator target, UAC | Integrity check before pasting | Not run |
| C08 | RDP, several users, sleep/resume, Microsoft cloud sync at the same time | | Not run |
| C09 | Pause, exclusions, clear removes data from disk | System clipboard test (pause), unit tests (exclusions, clear, save of the remaining list) | Passed |
| C10 | Idle CPU | `tools\bench.ps1`, 300 s | See benchmarks.md |
| UI-1 | Filters by keyboard (Tab, Tab, Right ×4 → Phone) | Key messages posted to the panel | Passed |
| UI-2 | Save image through the real Save dialog | Click on the save button, file name set in the dialog, OK | Passed: 750×584 PNG written, confirmation shown |
| UI-3 | Settings: change retention, shortcut, theme and save | Controls set by message, Save clicked | Passed: settings.ini updated, light theme and new shortcut applied immediately |
| UI-4 | Empty states (history off, nothing copied) | Screenshot | Passed |
| UI-5 | Emoji, kaomoji, symbols and GIF pages; insert an emoji | Clicks posted to the panel; first emoji typed into a RichTextBox window | Passed: the emoji arrived, the clipboard was unchanged, it is listed under recently used |
| UI-6 | Settings page: switch, invalid number, scrolling | Clicks and EN_KILLFOCUS posted to the window | Passed: the switch applied at once, the error is shown under the setting |
| SET | Setup file `BetterClipboard-Setup-0.1.0.exe` with "With Win+V" | Task dialog driven with TDM_CLICK_RADIO_BUTTON / TDM_CLICK_BUTTON | Passed: installed, started, Win+V owned by BetterClipboard |
| INS | Install, start, uninstall | `--install --quiet`, check shortcut, registry and Run key, `--uninstall --quiet`, check that everything is gone | See below |

## Manual checks still open

- Display scaling 125/150/200% and moving the panel between monitors with different scaling.
- Narrator with the panel open.
- Pasting into Word, Excel, Edge, Windows Terminal and Acrobat by hand.
- Pressing Win+V by hand after the takeover (the shortcut registration itself was verified).
- A real Android phone with Phone Link cross-device copy and paste.
