#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
command -v cargo >/dev/null || { echo 'cargo absent : installez rustup et initialisez Rust' >&2; exit 1; }
command -v x86_64-w64-mingw32-gcc >/dev/null || { echo 'MinGW absent : sudo pacman -S mingw-w64-gcc' >&2; exit 1; }
rustup target add x86_64-pc-windows-gnu
cargo test
cargo build --release
cargo build --release --target x86_64-pc-windows-gnu
printf '\nLinux : %s\nWindows : %s\n' "$PWD/target/release/sshbridge" "$PWD/target/x86_64-pc-windows-gnu/release/sshbridge.exe"
ls -lh target/release/sshbridge target/x86_64-pc-windows-gnu/release/sshbridge.exe
