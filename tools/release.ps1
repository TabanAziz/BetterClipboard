# Builds dist\BetterClipboard-<version>-portable.zip and SHA256SUMS.txt.
# Refuses to finish if the binary would reveal the builder's user name or profile path.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$version = ([regex]'(?m)^version = "([^"]+)"').Match((Get-Content (Join-Path $root 'Cargo.toml') -Raw)).Groups[1].Value

& (Join-Path $PSScriptRoot 'check.ps1')

$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$flags = @(
    '-Ctarget-feature=+crt-static',
    "--remap-path-prefix=$env:USERPROFILE=~",
    "--remap-path-prefix=$cargoHome=cargo",
    "--remap-path-prefix=$root=bcb"
)
$previous = $env:CARGO_ENCODED_RUSTFLAGS
$env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1f
Push-Location $root
try {
    & cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'build failed' }
}
finally {
    Pop-Location
    $env:CARGO_ENCODED_RUSTFLAGS = $previous
}

$app = Join-Path $root 'target\release\BetterClipboard.exe'
$latin = [Text.Encoding]::GetEncoding(28591)
$text = $latin.GetString([IO.File]::ReadAllBytes($app))
foreach ($private in @($env:USERPROFILE, "\$env:USERNAME\", "/$env:USERNAME/")) {
    foreach ($needle in @($private, $latin.GetString([Text.Encoding]::Unicode.GetBytes($private)))) {
        if ($text.IndexOf($needle, [StringComparison]::OrdinalIgnoreCase) -ge 0) { throw "$app contains '$private'" }
    }
}

$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$zipFile = Join-Path $dist "BetterClipboard-$version-portable.zip"
Remove-Item $zipFile -ErrorAction SilentlyContinue
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::Open($zipFile, 'Create')
try {
    $parts = [ordered]@{
        'BetterClipboard.exe'      = $app
        'LICENSE.txt'              = (Join-Path $root 'LICENSE')
        'THIRD-PARTY-LICENSES.txt' = (Join-Path $root 'THIRD-PARTY-LICENSES')
    }
    foreach ($name in $parts.Keys) {
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $parts[$name], $name) | Out-Null
    }
}
finally { $zip.Dispose() }
# The setup file is the same program; a name containing "Setup" makes it show the setup dialog.
$setupFile = Join-Path $dist "BetterClipboard-Setup-$version.exe"
Copy-Item $app $setupFile -Force
$sums = foreach ($file in @($setupFile, $zipFile)) {
    '{0}  {1}' -f (Get-FileHash $file -Algorithm SHA256).Hash.ToLower(), (Split-Path -Leaf $file)
}
[IO.File]::WriteAllText((Join-Path $dist 'SHA256SUMS.txt'), (($sums -join "`n") + "`n"))
"Release $version is in $dist"
$sums
