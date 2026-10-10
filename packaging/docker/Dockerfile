# 通用 Rust 编译镜像：同时编译 IOT_Power 与 Cross-SystemConnect（Linux x86_64 + Windows x86_64-pc-windows-gnu）。
# 基础镜像 cross-systemconnect-packager 已含 Rust 1.98.1（Debian bookworm）、mingw-w64、GTK/WebKit 开发库；
# 这里补上 IOT_Power 需要的部分。源码在运行时挂载到 /src，镜像里不放源码。
#   bash scripts/docker-build.sh --image
FROM cross-systemconnect-packager:latest

RUN apt-get update && apt-get install -y --no-install-recommends \
        libudev-dev libusb-1.0-0-dev \
        libxkbcommon-dev libwayland-dev libgl1-mesa-dev \
        nsis \
    && rm -rf /var/lib/apt/lists/* \
    && rustup component add clippy rustfmt \
    && cargo install cargo-deb --locked --root /usr/local/cargo \
    && rm -rf /usr/local/cargo/registry /usr/local/cargo/git

ENV CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
    CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
    AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar \
    WINDRES=x86_64-w64-mingw32-windres

WORKDIR /src
CMD ["bash"]
