<#
  Build the all-in-one Sidechain.exe (neural model, DirectML and the hook addon embedded; see build.rs).

    .\tools\package.ps1            # -> dist\Sidechain.exe
    .\tools\package.ps1 -Install   # ...and install it for this user:
                                   #    %LOCALAPPDATA%\Programs\Sidechain\Sidechain.exe + Start menu shortcut,
                                   #    this repo's sidechain.toml + presets copied to %APPDATA%\Sidechain (only if not there yet)

  Embedding needs models\htdemucs_fwd_fp16.onnx (tools\convert_fp16.py) and dist\sidechain-hook.vpk
  (build_addon.ps1) to exist; without them the exe still works, it just doesn't carry them.
#>
param([switch]$Install)
$ErrorActionPreference = "Stop"
$repo = Split-Path $PSScriptRoot -Parent
Set-Location $repo

foreach ($need in "models\htdemucs_fwd_fp16.onnx", "dist\sidechain-hook.vpk") {
    if (-not (Test-Path $need)) { Write-Warning "$need missing: it won't be embedded" }
}
# cargo reports progress on stderr, which Windows PowerShell would turn into a terminating error
$ErrorActionPreference = "Continue"
cargo build --release
$ErrorActionPreference = "Stop"
if ($LASTEXITCODE -ne 0) { throw "build failed" }
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\sidechain.exe dist\Sidechain.exe -Force
$mb = [math]::Round((Get-Item dist\Sidechain.exe).Length / 1MB)
Write-Output "==> dist\Sidechain.exe ($mb MB)"

if (-not $Install) { return }

if (Get-Process Sidechain -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "*Programs\Sidechain*" }) {
    throw "the installed Sidechain is running: quit it from the tray first"
}
$app = Join-Path $env:LOCALAPPDATA "Programs\Sidechain"
New-Item -ItemType Directory -Force $app | Out-Null
Copy-Item dist\Sidechain.exe (Join-Path $app "Sidechain.exe") -Force
Write-Output "==> installed $app\Sidechain.exe"

$cfg = Join-Path $env:APPDATA "Sidechain"
New-Item -ItemType Directory -Force $cfg | Out-Null
if (-not (Test-Path (Join-Path $cfg "sidechain.toml")) -and (Test-Path sidechain.toml)) {
    Copy-Item sidechain.toml $cfg
    Write-Output "==> settings copied to $cfg"
}
if (Test-Path presets) {
    New-Item -ItemType Directory -Force (Join-Path $cfg "presets") | Out-Null
    Get-ChildItem presets -Filter *.toml | ForEach-Object {
        $dst = Join-Path $cfg "presets\$($_.Name)"
        if (-not (Test-Path $dst)) { Copy-Item $_.FullName $dst }
    }
}

$lnk = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Sidechain.lnk"
$sh = New-Object -ComObject WScript.Shell
$s = $sh.CreateShortcut($lnk)
$s.TargetPath = Join-Path $app "Sidechain.exe"
$s.WorkingDirectory = $app
$s.Description = "Sidechain: Deadlock drives your music"
$s.Save()
Write-Output "==> Start menu shortcut: $lnk"
