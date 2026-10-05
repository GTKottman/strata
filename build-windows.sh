#!/usr/bin/env bash
# Cross-compiles Strata for 64-bit Windows and packages dist/Strata-windows.zip
# (Strata.vst3 bundle + Strata.clap). Needs rustup target x86_64-pc-windows-msvc and cargo-xwin.
set -euo pipefail
cd "$(dirname "$0")"
XWIN_ACCEPT_LICENSE=1 cargo xwin build --release --lib --target x86_64-pc-windows-msvc
rm -rf dist && mkdir -p dist/Strata.vst3/Contents/x86_64-win
cp target/x86_64-pc-windows-msvc/release/strata.dll dist/Strata.vst3/Contents/x86_64-win/Strata.vst3
cp target/x86_64-pc-windows-msvc/release/strata.dll dist/Strata.clap
(cd dist && zip -qr Strata-windows.zip Strata.vst3 Strata.clap)
echo "dist/Strata-windows.zip"
