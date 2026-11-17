#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
apifox_sysroot=$(rustc --print sysroot)
export APIFOX_BUILD_EXTRA_RUSTFLAGS="${RUSTFLAGS-}"
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$PWD=/workspace/apifox-helper-openidea --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$apifox_sysroot=/rust"
cargo build --locked --release --target wasm32-wasip2
mkdir -p dist
cp target/wasm32-wasip2/release/apifox_helper_openidea.wasm dist/plugin.wasm
if command -v wasm-tools >/dev/null 2>&1; then
    wasm-tools validate dist/plugin.wasm
fi
python3 scripts/build_info.py
python3 scripts/package.py
