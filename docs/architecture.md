# Architecture

BetterClipboard is a single native Win32 executable written in Rust on top of `windows-sys`. There is no UI framework, no web view and no runtime to install. The drawing layer (a 32-bit DIB back buffer with anti-aliased shapes and GDI text) is shared with Better Task Manager.

## Threads

| Thread | Owns | Talks to the UI through |
| --- | --- | --- |
| UI (main) | Hidden main window (hotkey, tray, timers), the panel, settings and snippet windows, the history list | — |
| clipboard | A message-only window registered with `AddClipboardFormatListener`; every `OpenClipboard`/`GetClipboardData`/`SetClipboardData` call; picture decoding, thumbnails and PNG encoding | `mpsc` channel + `PostMessage(WM_CLIP_EVENT)` |
| storage | Loading and saving `history.bin` (DPAPI) | channel + `PostMessage(WM_JOB)` |
| short-lived workers | Writing a saved or opened picture to disk | same as storage |

The UI thread never touches clipboard data or the disk. A clipboard owner that hangs while rendering delayed data stalls only the clipboard thread.

## Capturing

1. Windows posts `WM_CLIPBOARDUPDATE` to the listener window. There is no polling.
2. `GetClipboardSequenceNumber` filters repeats and BetterClipboard's own writes.
3. Copies marked as private (`ExcludeClipboardContentFromMonitorProcessing`, `Clipboard Viewer Ignore`, `CanIncludeInClipboardHistory = 0`) and copies from excluded apps are skipped.
4. The owner window's process (`GetClipboardOwner`) names the source app. `PhoneExperienceHost.exe` and the other Phone Link processes mark the entry as coming from a phone.
5. The clipboard is opened with a short retry; if another program keeps it open, a timer retries up to five times.
6. Formats read: `CF_UNICODETEXT`, `HTML Format`, `Rich Text Format`, `CF_HDROP` with `Preferred DropEffect`, and pictures from `PNG`, `CF_DIBV5` or `CF_DIB`.
7. After the clipboard is closed, pictures are decoded (`image.rs`), reduced to a 680×280 preview and kept as PNG bytes, so a screenshot costs its compressed size in memory.

## History model

`model.rs` is free of Win32 calls and fully unit tested: newest first, unpinned entries limited (25 by default), pinned entries never count against the limit, identical copies grouped, search over a lower-case key that is cut to 16 000 characters per entry.

## Pasting

1. The UI hides the panel and asks the clipboard thread to write the entry with all formats (or only text for "Paste as plain text").
2. When the write is confirmed, the UI checks the target: the window must still exist and must not run at a higher integrity level (UIPI would drop the input).
3. `SetForegroundWindow` returns focus to the target; up to twelve 15 ms retries if Windows needs a moment.
4. A normal Ctrl+V is sent with `SendInput`. Modifiers still held from the shortcut are released first, with a dummy key so a released Windows key does not open Start.

## Storage and privacy

- `settings.ini`: plain text, no clipboard content.
- `history.bin`: `BCBH` magic, version, then a `CryptProtectData` blob (current user, extra entropy, no UI). Written to a temporary file, flushed, then renamed over the old file. A file that cannot be decrypted or parsed is moved aside, not deleted.
- By default only pinned entries are written. "Keep unpinned entries" and "Save pinned entries" can change that, down to nothing on disk at all.
- No logging of clipboard data. No network access.

## Windows integration

- Shortcut through `RegisterHotKey` (default Win+Alt+V). Win+V can only be used after Explorer's own shortcut is switched off through the documented `DisabledHotkeys` value, which applies after signing out. Uninstalling undoes it if BetterClipboard made the change.
- Tray icon (`Shell_NotifyIconW`, version 4), recreated on `TaskbarCreated`.
- `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` (out of context) remembers the last real app window, so the panel opened from the tray knows where to paste. No keyboard hooks.
- Per-user install under `%LOCALAPPDATA%\Programs\BetterClipboard` with a Start menu shortcut (`IShellLinkW`), an "Installed apps" entry and the `Run` key. The release ships the same executable a second time as `BetterClipboard-Setup-<version>.exe`; a file name containing "Setup" shows a task dialog (`TaskDialogIndirect`) with the Win+V choice.
- Emoji, kaomoji and symbols are typed into the previous app with `SendInput` and `KEYEVENTF_UNICODE`, so inserting them never changes the clipboard.
- The settings window draws its cards, switches and segmented control into the shared canvas; text fields are standard edit controls placed on top.

## Performance budget

| Path | Budget |
| --- | --- |
| Shortcut until the painted panel | p95 ≤ 80 ms (25 entries), ≤ 150 ms (1 000 entries) |
| Keystroke in search until painted, 10 000 entries | p95 ≤ 50 ms |
| Idle CPU | < 0.5 % of one core |
| Start-up until ready | p95 ≤ 150 ms warm |

Measured results are in [benchmarks.md](benchmarks.md).
