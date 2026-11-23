# Contributing

Bug reports and pull requests are welcome.

## Build and check

Requirements: Windows 10 or 11 (x64), Rust stable with the MSVC toolchain. Python 3 with Pillow is only needed if you change the icon (`py tools\make_icon.py`).

    cargo build --release
    powershell -File tools\check.ps1

`tools\check.ps1` runs rustfmt, Clippy with warnings as errors, the unit tests and a release build.

The tests in `tests\system_clipboard.rs` use the real Windows clipboard and overwrite what is on it, so they only run on request:

    cargo test --test system_clipboard -- --ignored --test-threads=1

Benchmarks: `powershell -File tools\bench.ps1` (see `docs\benchmarks.md`).

## Guidelines

- Keep the UI thread free: clipboard access runs on the clipboard thread, disk access on the storage thread.
- Never write clipboard content to logs.
- Behaviour that mirrors Windows gets a row in `docs/parity-matrix.csv` with its test.
- Keep pull requests focused and run `tools\check.ps1` before sending them.
