# Security

BetterClipboard handles everything you copy, so security reports are taken seriously.

Please report vulnerabilities privately with GitHub's "Report a vulnerability" button on this repository instead of opening a public issue. Include the version, the Windows build and the steps to reproduce.

What the app does and does not do:

- Clipboard data stays on your PC. There is no network code and no telemetry.
- Saved entries are encrypted with DPAPI for your Windows account (`%LOCALAPPDATA%\BetterClipboard\history.bin`).
- Copies that apps mark as private (`ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory = 0`, `Clipboard Viewer Ignore`) are never recorded.
- No keyboard hooks, no DLL injection, no elevation. Pasting into apps that run as administrator is blocked by Windows; BetterClipboard reports that instead of working around it.
