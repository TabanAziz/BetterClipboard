# Benchmarks

All numbers below were measured once on one PC with the build of commit ea0e257 (before the emoji pages and the new settings window, which do not touch the measured paths). They are not averages across machines.

## Reference PC

| | |
| --- | --- |
| Device | Laptop on mains power, Intel Core i7-11800H (8 cores, 16 threads), 16 GB RAM, WD PC SN540 NVMe SSD |
| Graphics | Intel UHD Graphics and NVIDIA GeForce RTX 3070 Laptop GPU |
| Display | 1920×1080, 100% scaling, one monitor, 144 Hz |
| Windows | Windows 11 Pro 25H2, build 10.0.26200.9974, de-AT region, dark app theme, Microsoft Defender active |
| Build | `cargo build --release` (Rust 1.99.1, fat LTO, static CRT), 1 060 352 bytes |
| Load | Normal desktop use: a browser and other apps were open during the runs |

## How to run

    cargo build --release
    powershell -File tools\bench.ps1 -Runs 30 -IdleSeconds 300

The script uses its own profile (`BETTERCLIPBOARD_PROFILE=bench`), never registers the shortcut and deletes its data afterwards. Raw data lands in `bench\out`; the data behind this page is in [bench-data](bench-data).

## What is measured

| Measurement | Start | End |
| --- | --- | --- |
| Start-up (warm) | Process creation time (`GetProcessTimes`) | Windows created, tray icon added, clipboard listener running, message queue empty (`--startup-report`; the run uses `--no-hotkey`, so the single `RegisterHotKey` call is left out) |
| Panel open | The same function the shortcut calls (`show_popup`) | `WM_PAINT` finished and `DwmFlush` returned, i.e. the frame was handed to the compositor (`--bench-popup`) |
| Search keystroke | A character typed into the panel | The filtered list painted and flushed (`--bench-search`, query typed letter by letter, 4 rounds) |
| Idle | 5 s after start | `IdleSeconds` later: CPU time of the process, working set, threads, handles |

The panel numbers start at the hotkey message inside the app. The time Windows needs to deliver the key press to the app is not included; it is the same for any program. `DwmFlush` waits for the next composition pass, so at 144 Hz up to about 7 ms of every panel number is waiting for the screen.

Cold start after a reboot was not measured. A comparison with Win+V was not measured either, because it needs synthetic Win+V key presses on a desktop that was in use.

## Results

| Measurement | n | min | p50 | p95 | p99 | max |
| --- | --- | --- | --- | --- | --- | --- |
| Start-up, warm | 30 | 26.2 ms | 27.9 ms | 35.4 ms | 45.5 ms | 45.5 ms |
| Panel open, 25 entries | 30 | 7.3 ms | 19.1 ms | 32.6 ms | 32.8 ms | 32.8 ms |
| Panel open, 1 000 entries | 30 | 7.8 ms | 18.2 ms | 35.8 ms | 37.3 ms | 37.3 ms |
| Search keystroke, 10 000 entries | 68 | 6.6 ms | 13.6 ms | 14.2 ms | 20.3 ms | 20.7 ms |

Idle over 300 seconds: CPU time did not advance at all (0 ms; Windows counts process CPU time in 15.6 ms steps, so the real value is below 0.006 % of one core), 11.5 MB working set, 2.0 MB private memory, 7 threads, 148 handles. The clipboard listener only wakes up when something is copied; there are no timers except an hourly retention check.

Budgets from [architecture.md](architecture.md): panel p95 ≤ 80 ms (met), 1 000 entries p95 ≤ 150 ms (met), search p95 ≤ 50 ms (met), warm start p95 ≤ 150 ms (met), idle CPU < 0.5 % of one core (met).

For comparison, the Windows panel is hosted in `TextInputHost.exe`, which used 155 MB working set and 165 MB private memory on the same PC at the time of the measurement. That process also hosts the touch keyboard and emoji panel, so the numbers are not a like-for-like comparison.
