@echo off
rem Run sidechain from the repo: uses .\sidechain.toml (created from the defaults on first run).
cd /d "%~dp0"
if not exist sidechain.toml copy sidechain.default.toml sidechain.toml >nul
if not exist target\release\sidechain.exe cargo build --release || exit /b 1
target\release\sidechain.exe %*
