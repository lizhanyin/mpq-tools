@echo off
setlocal
cd /d "%~dp0"

echo [1/2] Building frontend (gui/ui)...
call npm --prefix gui/ui run build
if errorlevel 1 (
    echo Frontend build failed, aborting.
    exit /b 1
)

echo [2/2] Building and launching mpqtool-gui...
cargo run -p mpqtool-gui
