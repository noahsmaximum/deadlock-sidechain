<#
    Sidechain hook addon - build.

    Pipeline (CSDK 12):  addon/ -> CSDK content/ -> resourcecompiler -> CSDKCfgVPK -> dist\sidechain-hook.vpk
    Install the VPK with Deadlock Mod Manager as a local mod (it handles pak numbering).

    Usage:
        .\build_addon.ps1          # sync + compile + pack (safe while the game is running)
        .\build_addon.ps1 -Deploy  # ...and overwrite every installed copy (Mod Manager profiles). GAME MUST BE CLOSED.

    Footguns (from the cinema-cam / boss-rush builds):
      * Compile each source in its OWN resourcecompiler invocation; tight batches silently drop files.
      * The layout override is the CURRENT game's citadel_hud_top_bar.xml + one <scripts> include.
        After a Deadlock patch, re-decompile it (Source2Viewer-CLI) and re-apply the include.
      * Keep this file pure ASCII: Windows PowerShell 5.1 reads it as ANSI.
#>
[CmdletBinding()]
param(
    [switch]$Deploy,
    [string]$CsdkRoot = "C:\Users\Noah\Projects\Reduced_CSDK_12",
    [string]$Addons   = "F:\I Drive\SteamLibrary\steamapps\common\Deadlock\game\citadel\addons"
)

$ErrorActionPreference = "Stop"
$AddonName = "sidechain"
$Src       = Join-Path $PSScriptRoot "addon"
$Content   = Join-Path $CsdkRoot "content\citadel_addons\$AddonName"
$GameOut   = Join-Path $CsdkRoot "game\citadel_addons\$AddonName"
$RC        = Join-Path $CsdkRoot "game\bin\win64\resourcecompiler.exe"
$Packer    = Join-Path $CsdkRoot "game\bin\win64\CSDKCfgVPK.exe"
$CsdkCitadel = Join-Path $CsdkRoot "game\citadel"

foreach ($t in @($RC, $Packer)) { if (-not (Test-Path $t)) { throw "missing build tool: $t" } }

Write-Host "==> sync  $Src  ->  $Content" -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $Content | Out-Null
robocopy $Src $Content /MIR /NFL /NDL /NJH /NJS /NP | Out-Null
if ($LASTEXITCODE -ge 8) { throw "robocopy failed ($LASTEXITCODE)" }

if (Test-Path $GameOut) { Remove-Item $GameOut -Recurse -Force }
$sources = Get-ChildItem $Content -Recurse -File -Include *.xml, *.js
Write-Host "==> compiling $($sources.Count) source(s)" -ForegroundColor Cyan
$failed = @()
foreach ($s in $sources) {
    $rel = $s.FullName.Substring($Content.Length + 1)
    $out = & $RC -f -danger_mode_ignore_schema_mismatches -game $CsdkCitadel -i $s.FullName 2>&1
    $summary = $out | Where-Object { $_ -match "error|failed|OK:|compiled" } | Select-Object -Last 2
    Write-Host ("    {0,-44} {1}" -f $rel, ($summary -join " | "))
    if ($LASTEXITCODE -ne 0 -or -not ($out -match "1 compiled")) { $failed += $rel }
}
if ($failed.Count) { throw "compile failed for: $($failed -join ', ')" }
Copy-Item (Join-Path $Src "addoninfo.txt") $GameOut -Force

$dist = Join-Path $PSScriptRoot "dist"
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$vpkOut = Join-Path $dist "sidechain-hook.vpk"
if (Test-Path $vpkOut) { Remove-Item $vpkOut -Force }
Write-Host "==> packing  $GameOut  ->  $vpkOut" -ForegroundColor Cyan
$p = Start-Process -FilePath $Packer -ArgumentList @("`"$GameOut`"", "`"$vpkOut`"") -NoNewWindow -PassThru
Start-Sleep -Seconds 6
if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
if (-not (Test-Path $vpkOut)) { throw "packer produced no VPK at $vpkOut" }
Write-Host ("    VPK ok ({0:N0} bytes)" -f (Get-Item $vpkOut).Length) -ForegroundColor Green

if (-not $Deploy) {
    Write-Host "==> first install: add dist\sidechain-hook.vpk in Deadlock Mod Manager as a local mod. Updates: -Deploy." -ForegroundColor Yellow
    return
}
if (Get-Process -Name "deadlock" -ErrorAction SilentlyContinue) {
    throw "Deadlock is running. Close it first: installed VPKs are file-locked while the game is up."
}
# Every installed copy = any addon VPK that contains our script (Mod Manager renames/copies them).
# Only small VPKs can be ours (~20 KB); the addons folder can hold many GB of other mods.
$needle = [Text.Encoding]::ASCII.GetBytes("sidechain_hook")
$installed = Get-ChildItem $Addons -Recurse -Filter *.vpk | Where-Object { $_.Length -lt 1MB } | Where-Object {
    $bytes = [IO.File]::ReadAllBytes($_.FullName)
    ([Text.Encoding]::ASCII.GetString($bytes)).Contains("sidechain_hook")
}
if (-not $installed) { throw "no installed copy found under $Addons - add dist\sidechain-hook.vpk in Mod Manager first" }
foreach ($f in $installed) {
    Copy-Item $vpkOut $f.FullName -Force
    Write-Host "==> DEPLOYED -> $($f.FullName)" -ForegroundColor Green
}
