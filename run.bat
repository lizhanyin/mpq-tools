@echo off
setlocal
cd /d "%~dp0"

rem 结束运行中的旧实例（否则 exe 被占用，链接阶段会失败；未运行时忽略）
taskkill /IM mpqtool-gui.exe /F >nul 2>&1

echo [1/2] Building frontend (gui/ui)...
call npm --prefix gui/ui run build
if errorlevel 1 (
    echo Frontend build failed, aborting.
    exit /b 1
)

echo [2/2] Building and launching mpqtool-gui...
cargo run -p mpqtool-gui
