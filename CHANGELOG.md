# Changelog

## 0.1.4

- Settings now previews six BetterExplorer themes alongside System, Light and Dark. The selected theme applies immediately to the clipboard panel and Settings and is kept after a restart.

## 0.1.3

- Phone items no longer come back to the phone in the common cases. Phone Link sends every change of the PC clipboard to the phone and ignores the "do not process" markers, so BetterClipboard now avoids changing the clipboard for phone items:
  - choosing the phone item that is still on the clipboard pastes it without writing anything;
  - older text from the phone is typed into the app instead of going through the clipboard.
- Older pictures from the phone still have to go through the clipboard, so Phone Link may send those back.

## 0.1.2

- The emoji page is instant: colour emoji are rendered once on a background thread and then only copied. First opening went from about 400 ms to about 2 ms, a repaint while moving the mouse from about 31 ms to under 1 ms.
- Items that came from your phone are no longer sent back to it when you paste them on the PC (they are marked for clipboard monitors, Windows history and cloud sync to ignore).
- Pasting waits until the target app has its text field focused again, at most 0.3 s.
- Benchmark runs shut down their worker threads cleanly.

## 0.1.1

- Emoji are drawn in colour, like in the Windows panel (DirectWrite instead of GDI).
- The GIF tab is gone: GIF search needs an online service, and BetterClipboard stays offline.

## 0.1.0

First public version.

- Clipboard history panel laid out like the Windows 11 Win+V panel, opened with Win+Alt+V (configurable) or the tray icon.
- Text, links, HTML, RTF, pictures and files, written back with all their formats.
- Picture previews, "Save image as" (PNG or BMP) and "Open image".
- Entries that arrive from an Android phone through Phone Link are marked and have their own filter.
- Pin, delete, clear all (keeps pinned), search, filters, duplicate grouping, snippets.
- Pinned entries saved encrypted with DPAPI; saving the whole history is optional.
- Pause, excluded apps, retention period, and respect for apps that mark copies as private.
- Per-user install and uninstall without administrator rights.
- Setup file with a choice to open the history with Win+V.
- Emoji, kaomoji, symbols and recently used pages in the panel.
- Settings redesigned after Windows 11 Settings, with changes applied at once.
- `--use-win-v` / `--windows-win-v` move Win+V between Windows and BetterClipboard, with an Explorer restart so no sign-out is needed.
