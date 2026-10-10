#!/usr/bin/env bash
# 在编译镜像内运行：格式、clippy、单元测试，并确认 Windows 目标能编译。
set -euo pipefail
cd /src

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo check --locked --target x86_64-pc-windows-gnu -p iot-power
