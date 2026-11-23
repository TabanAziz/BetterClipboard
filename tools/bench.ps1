# Runs the benchmarks described in docs/benchmarks.md and writes raw results to bench\out.
# Uses a separate profile ("bench"), so your own history and settings are not touched.
param([int]$Runs = 30, [int]$IdleSeconds = 300)
$ErrorActionPreference = 'Stop'
# Dots as decimal separators in every number written below.
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\release\BetterClipboard.exe'
$out = Join-Path $root 'bench\out'
New-Item -ItemType Directory -Force $out | Out-Null
$env:BETTERCLIPBOARD_PROFILE = 'bench'
$profileDir = Join-Path $env:LOCALAPPDATA 'BetterClipboard\profiles\bench'

function Reset-Profile {
    & $exe --quit
    Start-Sleep -Milliseconds 500
    Remove-Item $profileDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $profileDir | Out-Null
    # An existing settings file keeps the first-run notification away.
    'history_enabled=1' | Set-Content (Join-Path $profileDir 'settings.ini') -Encoding ascii
}

function Summary([double[]]$values) {
    $s = $values | Sort-Object
    $pick = { param($p) $s[[math]::Min($s.Count - 1, [math]::Round(($p / 100) * ($s.Count - 1)))] }
    '{0} runs: min {1:N1} / p50 {2:N1} / p95 {3:N1} / p99 {4:N1} / max {5:N1} ms' -f $s.Count, $s[0], (& $pick 50), (& $pick 95), (& $pick 99), $s[-1]
}

# Start-up: process creation until hotkey, tray icon and listener are ready and the queue is idle.
Reset-Profile
$startup = @()
for ($i = 0; $i -lt $Runs; $i++) {
    $file = Join-Path $out "startup-$i.txt"
    Start-Process $exe -ArgumentList "--no-hotkey --startup-report `"$file`"" -Wait
    $startup += [double](Get-Content $file)
    Remove-Item $file
}
$startup | Set-Content (Join-Path $out 'startup.txt')
"startup (warm): $(Summary $startup)"

# Panel: open request until the painted frame reached the compositor.
foreach ($items in 25, 1000) {
    Reset-Profile
    $file = Join-Path $out "popup-$items.txt"
    Start-Process $exe -ArgumentList "--no-hotkey --bench-popup $Runs --items $items --report `"$file`"" -Wait
    (Get-Content $file)[1]
}

# Search: one keystroke until the filtered list was painted, 10 000 entries.
Reset-Profile
$file = Join-Path $out 'search-10000.txt'
Start-Process $exe -ArgumentList "--no-hotkey --bench-search --items 10000 --report `"$file`"" -Wait
(Get-Content $file)[1]

# Idle: CPU time and memory of a running instance that is left alone.
Reset-Profile
$p = Start-Process $exe -ArgumentList '--no-hotkey --background' -PassThru
Start-Sleep -Seconds 5
$p.Refresh(); $cpu0 = $p.TotalProcessorTime.TotalMilliseconds
Start-Sleep -Seconds $IdleSeconds
$p.Refresh(); $cpu1 = $p.TotalProcessorTime.TotalMilliseconds
$idle = [ordered]@{
    seconds = $IdleSeconds
    cpu_ms = [math]::Round($cpu1 - $cpu0, 1)
    cpu_percent_of_one_core = [math]::Round(100 * ($cpu1 - $cpu0) / ($IdleSeconds * 1000), 4)
    working_set_mb = [math]::Round($p.WorkingSet64 / 1MB, 1)
    private_mb = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
    threads = $p.Threads.Count
    handles = $p.HandleCount
}
$idle | ConvertTo-Json | Set-Content (Join-Path $out 'idle.json')
"idle: $($idle | ConvertTo-Json -Compress)"
& $exe --quit
Start-Sleep -Milliseconds 500
Remove-Item $profileDir -Recurse -Force -ErrorAction SilentlyContinue
