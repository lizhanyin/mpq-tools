#!/bin/sh
set -e
cd "$(dirname "$0")"

echo "[1/2] Building frontend (gui/ui)..."
npm --prefix gui/ui run build

echo "[2/2] Building and launching mpqtool-gui..."
cargo run -p mpqtool-gui
