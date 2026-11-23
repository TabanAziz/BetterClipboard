# Everything that has to pass before a commit or a release.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

function Invoke-Cargo {
    & cargo @args
    if ($LASTEXITCODE -ne 0) { throw "cargo $($args -join ' ') failed" }
}

Push-Location $root
try {
    Invoke-Cargo fmt --check
    Invoke-Cargo clippy --release --locked --all-targets '--' -D warnings
    Invoke-Cargo test --release --locked
    Invoke-Cargo build --release --locked
    'All checks passed.'
}
finally { Pop-Location }
