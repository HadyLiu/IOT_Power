#!/usr/bin/env bash
# 在编译镜像内运行：生成 Linux 与 Windows 发布程序，输出到 dist/。
set -euo pipefail
cd /src

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"

cargo build --release --locked -p iot-power
cargo build --release --locked --target x86_64-pc-windows-gnu -p iot-power

rm -rf dist/linux dist/windows
mkdir -p dist/linux dist/windows
cp -f target/release/iot-power dist/linux/
cp -f target/x86_64-pc-windows-gnu/release/iot-power.exe dist/windows/

linux_pkg="dist/IOT_Power-${version}-linux-x86_64.tar.gz"
win_pkg="dist/IOT_Power-${version}-windows-x64.zip"
rm -f "$linux_pkg" "$win_pkg"
tar -C dist/linux -czf "$linux_pkg" .
(cd dist/windows && zip -qr "../$(basename "$win_pkg")" .)

deb_pkg=""
if command -v cargo-deb >/dev/null 2>&1; then
  rm -f target/debian/*.deb
  cargo deb -p iot-power --no-build
  deb_pkg="$(ls target/debian/*.deb | head -n1)"
  cp -f "$deb_pkg" dist/
  deb_pkg="dist/$(basename "$deb_pkg")"
else
  echo "cargo-deb not installed, skipping .deb" >&2
fi
(cd dist && sha256sum ./*.tar.gz ./*.zip ./*.deb 2>/dev/null > SHA256SUMS || true)

if [[ -n "${HOST_UID:-}" && -n "${HOST_GID:-}" ]]; then
  chown -R "${HOST_UID}:${HOST_GID}" dist
fi

echo "Linux:   dist/linux/iot-power, $linux_pkg"
echo "Windows: dist/windows/iot-power.exe, $win_pkg"
[[ -n "$deb_pkg" ]] && echo "Deb:     $deb_pkg"
echo "SHA256:  dist/SHA256SUMS"
