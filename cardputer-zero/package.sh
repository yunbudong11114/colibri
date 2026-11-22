#!/bin/sh
set -eu
app_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ "$(uname -m)" != aarch64 ]; then
    echo "Build on Debian 13 ARM64 with a Rust toolchain." >&2
    exit 1
fi
build="$app_dir/build/arm64"
mkdir -p "$build" "$app_dir/dist"
cmake -S "$app_dir" -B "$build" -G Ninja -DUSE_DESKTOP=OFF -DCMAKE_BUILD_TYPE=Release \
    -DCOLIBRI_RUST_WORKER="${COLIBRI_RUST_WORKER:-}" \
    -DCOLIBRI_OFFLINE_SPEECH_PAYLOAD="${COLIBRI_OFFLINE_SPEECH_PAYLOAD:-}"
cmake --build "$build"
cpack --config "$build/CPackConfig.cmake" -B "$app_dir/dist"
