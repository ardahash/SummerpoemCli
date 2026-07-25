param(
    [string]$Version = "",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $root

if ($Version -eq "") {
    $match = Select-String -Path "Cargo.toml" -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    if ($null -eq $match) {
        throw "Could not read workspace version from Cargo.toml"
    }
    $Version = $match.Matches[0].Groups[1].Value
}

if (-not $SkipBuild) {
    $env:RUSTFLAGS = "-C target-feature=+crt-static"
    cargo build --release -p sump
}

$exe = Join-Path $root "target\release\sump.exe"
if (-not (Test-Path $exe)) {
    throw "Missing $exe. Build first, or rerun without -SkipBuild."
}

$distRoot = Join-Path $root "dist"
$pkgName = "summerpoem-v$Version-windows-x64"
$pkg = Join-Path $distRoot $pkgName
$zip = Join-Path $distRoot "$pkgName.zip"

New-Item -ItemType Directory -Force -Path $distRoot | Out-Null
Remove-Item -Recurse -Force $pkg -ErrorAction SilentlyContinue
Remove-Item -Force $zip -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $pkg | Out-Null

Copy-Item $exe (Join-Path $pkg "sump.exe") -Force

@'
@echo off
setlocal
cd /d "%~dp0"
if not exist "%~dp0wallet.json" "%~dp0sump.exe" wallet new --wallet "%~dp0wallet.json"
"%~dp0sump.exe" --chain-dir "%~dp0sumpchain" node run --mine --gpu --gui --wallet "%~dp0wallet.json" --connect seed.summerpoem.org:8776
pause
'@ | Set-Content -Encoding ASCII (Join-Path $pkg "Start Mining.bat")

@'
@echo off
setlocal
cd /d "%~dp0"
"%~dp0sump.exe" --chain-dir "%~dp0sumpchain" wallet balance --wallet "%~dp0wallet.json"
pause
'@ | Set-Content -Encoding ASCII (Join-Path $pkg "Check Balance.bat")

@"
Summerpoem v$Version Windows x64

Start mining:
1. Unzip this folder.
2. Double-click Start Mining.bat.
3. The launcher creates wallet.json if needed, connects to seed.summerpoem.org:8776, syncs, then mines.
4. Open the dashboard URL printed in the terminal, usually http://127.0.0.1:8787.

Check your balance:
- Double-click Check Balance.bat.

Important:
- Keep wallet.json private and backed up.
- GPU mining requires an NVIDIA CUDA-capable GPU and driver. If GPU mining is unavailable, the node falls back to CPU.
- The node must connect to public peers before mainnet mining starts.

Mainnet genesis:
60235b421eb3478072192851a1ea05eeb221dd8821aeaacb3fcd361abb21ca0d

Public bootstrap seed:
seed.summerpoem.org:8776
"@ | Set-Content -Encoding ASCII (Join-Path $pkg "QUICKSTART.txt")

Compress-Archive -Path (Join-Path $pkg "*") -DestinationPath $zip -Force

$hash = Get-FileHash $zip -Algorithm SHA256
Write-Host "Created $zip"
Write-Host "SHA256 $($hash.Hash)"

if (Get-Command dumpbin -ErrorAction SilentlyContinue) {
    Write-Host ""
    Write-Host "Dependencies:"
    dumpbin /dependents (Join-Path $pkg "sump.exe")
}
