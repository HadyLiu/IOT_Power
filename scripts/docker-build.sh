#!/usr/bin/env bash
# 用编译镜像 rust-cross-builder 编译 IOT_Power 或 Cross-SystemConnect。
# 镜像已存在则复用；不存在或传 --image 时才构建（基于 cross-systemconnect-packager:latest）。
#   bash scripts/docker-build.sh               # IOT_Power：Linux + Windows → dist/
#   bash scripts/docker-build.sh check         # IOT_Power：fmt + clippy + test + Windows check
#   bash scripts/docker-build.sh csc           # Cross-SystemConnect：Linux + Windows → <csc>/dist/
#   bash scripts/docker-build.sh shell         # 进入容器（源码挂载在 /src）
#   bash scripts/docker-build.sh --image [...] # 先重建镜像
# Cross-SystemConnect 路径默认 ../Cross-SystemConnect，可用环境变量 CSC_DIR 覆盖。
set -euo pipefail
cd "$(dirname "$0")/.."

image=rust-cross-builder:latest
base=cross-systemconnect-packager:latest

rebuild=0
if [[ "${1:-}" == "--image" ]]; then
  rebuild=1
  shift
fi
task="${1:-build}"
if [[ $# -gt 1 ]]; then
  echo "Usage: bash scripts/docker-build.sh [--image] [build|check|csc|shell]" >&2
  exit 1
fi

if [[ "$rebuild" -eq 0 ]] && ! docker image inspect "$image" >/dev/null 2>&1; then
  rebuild=1
fi
if [[ "$rebuild" -eq 1 ]]; then
  if ! docker image inspect "$base" >/dev/null 2>&1; then
    echo "Base image $base not found (build it in Cross-SystemConnect: bash scripts/package-docker.sh --image)" >&2
    exit 1
  fi
  echo "Building image $image from $base"
  docker build -t "$image" packaging/docker
else
  echo "Using existing image $image"
fi

iot_run() {
  local tty=()
  [[ -t 0 ]] && tty=(-it)
  docker run --rm "${tty[@]}" \
    -v "$PWD:/src" \
    -v iot-power-cargo-registry:/usr/local/cargo/registry \
    -v iot-power-cargo-git:/usr/local/cargo/git \
    -v iot-power-target:/src/target \
    -w /src \
    -e "HOST_UID=$(id -u)" \
    -e "HOST_GID=$(id -g)" \
    "$image" "$@"
}

case "$task" in
  build)
    mkdir -p dist
    iot_run bash packaging/docker/compile.sh
    ;;
  check)
    iot_run bash packaging/docker/check.sh
    ;;
  shell)
    iot_run bash
    ;;
  csc)
    csc_dir="$(cd "${CSC_DIR:-../Cross-SystemConnect}" && pwd)"
    mkdir -p "$csc_dir/dist"
    # 沿用 Cross-SystemConnect 自己的缓存卷与编译脚本
    docker run --rm \
      -v "$csc_dir:/src" \
      -v cross-systemconnect-cargo-registry:/usr/local/cargo/registry \
      -v cross-systemconnect-cargo-git:/usr/local/cargo/git \
      -v cross-systemconnect-target:/src/target \
      -w /src \
      -e "HOST_UID=$(id -u)" \
      -e "HOST_GID=$(id -g)" \
      "$image" \
      bash packaging/compile.sh
    ;;
  *)
    echo "Unknown task: $task (build|check|csc|shell)" >&2
    exit 1
    ;;
esac
