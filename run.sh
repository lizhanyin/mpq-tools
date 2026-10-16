#!/bin/sh
set -e
cd "$(dirname "$0")"

# 结束运行中的旧实例（否则 exe 被占用，链接阶段会失败；未运行时忽略）
if command -v taskkill >/dev/null 2>&1; then
  taskkill //IM mpqtool-gui.exe //F >/dev/null 2>&1 || true
else
  pkill -f 'mpqtool-gui' 2>/dev/null || true
fi

echo "[1/2] Building frontend (gui/ui)..."
npm --prefix gui/ui run build

echo "[2/2] Building and launching mpqtool-gui..."
cargo run -p mpqtool-gui
