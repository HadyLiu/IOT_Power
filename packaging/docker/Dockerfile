# 通用 Rust 编译镜像：同时编译 IOT_Power 与 Cross-SystemConnect（Linux x86_64 + Windows x86_64-pc-windows-gnu）。
# 自包含：直接基于官方 rust 镜像，本机与 GitHub Actions 用同一份 Dockerfile 构建，不再依赖本机私有的基础镜像。
# 源码在运行时挂载到 /src，镜像里不放源码。
#   本机：   bash scripts/docker-build.sh --image
#   GitHub： .github/workflows/docker-image.yml 构建并推送到 ghcr.io/hadyliu/iot-power-builder
FROM rust:1-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config zip \
        gcc-mingw-w64-x86-64 \
        libudev-dev libusb-1.0-0-dev \
        libxkbcommon-dev libwayland-dev libgl1-mesa-dev \
        nsis \
        libwebkit2gtk-4.1-dev libgtk-3-dev \
        libx11-dev libxi-dev libxtst-dev libxfixes-dev libxrandr-dev \
        libxdo-dev libayatana-appindicator3-dev \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-pc-windows-gnu \
    && rustup component add clippy rustfmt \
    && cargo install cargo-deb --locked --root /usr/local/cargo \
    && rm -rf /usr/local/cargo/registry /usr/local/cargo/git

ENV CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
    CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
    AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar \
    WINDRES=x86_64-w64-mingw32-windres

WORKDIR /src
CMD ["bash"]
