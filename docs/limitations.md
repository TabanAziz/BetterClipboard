# Known limitations

BetterClipboard is not a full replacement for the Windows clipboard panel. These are the differences and gaps that are known today.

## Platform limits

- **Win+V belongs to Windows by default.** Explorer registers it, so the default shortcut is Win+Alt+V. `BetterClipboard.exe --use-win-v` (or Settings > "Use Win+V...") switches off Explorer's Win+V through the documented `DisabledHotkeys` value and restarts Explorer. On the test PC Win+V was then free and owned by BetterClipboard. While it is switched off, the Windows panel is not reachable by keyboard; `--windows-win-v` gives it back.
- **No access to Windows' own history or cloud sync.** Windows' history and "Sync across your devices" are separate. BetterClipboard records from the moment it runs and cannot read entries copied before it started or synced from other PCs, unless Windows also puts them on the current clipboard. Both histories run side by side without conflict.
- **Apps running as administrator.** Windows blocks input from normal apps into elevated ones. BetterClipboard copies the entry and asks you to press Ctrl+V. It never requests elevation.
- **Secure desktop (UAC prompts, sign-in screen).** The panel cannot open there.

## Phone Link

- Phone entries are recognised by the process that writes them (`PhoneExperienceHost.exe` and related Phone Link processes). That only works when "Cross-device copy and paste" is turned on in Phone Link, which Phone Link supports for certain Android phones; iPhones are not supported by that feature.
- The detection was tested by writing to the clipboard from a process with the Phone Link file name, not with a real phone. If a future Phone Link version writes from a differently named process, its entries show up as normal entries.

## Differences from the Windows panel

- The emoji, GIF, kaomoji and symbol tabs open the Windows emoji panel (Win+.) in the previous app instead of switching pages inside the panel.
- The panel uses a solid background instead of acrylic and has no opening animation.
- The interface is in English only.
- Pictures are counted by their compressed size and may be up to 32 MB, so full-screen screenshots of 4K and 5K displays are kept. Windows counts the uncompressed size against 4 MB.
- File copies from Explorer are recorded (Windows does not show them).
- An immediate repeat of the same copy within 1.5 seconds counts once, because some programs announce one copy twice.

## Accessibility

- The panel is custom drawn. It announces the selected entry through its window title, but it does not implement a UI Automation tree, so screen readers cannot browse entries or buttons individually. The settings and snippet windows use standard controls.
- Keyboard: arrows, Page Up/Down, Home/End, Enter, Shift+Enter, Ctrl+Enter, Delete, Ctrl+P, Ctrl+S, Ctrl+F, Tab, Escape and the Menu key work. The tab row (emoji, GIF and so on) is mouse only.
- In dark mode the settings window uses the dark styles Windows offers for classic controls; checkboxes fall back to the classic look.

## Not verified on the test PC

- Display scaling other than 100% (the code is per-monitor DPI aware), multiple monitors, RDP sessions, fast user switching, sleep and resume.
- Pasting into Office, PDF readers and terminals by hand; the automated paste test used a RichTextBox, the format tests used the clipboard directly.
- Cold start after a reboot (only warm starts were measured).
- A comparison of panel opening time against Win+V: measuring it needs synthetic Win+V key presses on the desktop, which was not done.

## Not implemented

- Folders or categories for entries.
- Any own sync between devices.
- Signed releases and an SBOM.
