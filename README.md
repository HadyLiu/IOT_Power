# IOT_Power

基于 **Rust + egui/eframe** 的跨平台桌面上位机，用于控制 **DC3005U** 可编程电源 / 功耗分析仪：实时读取电压、电流、功率，绘制曲线、录制数据，设定输出，并提供预设、序列、扫描、函数发生器、电池模拟等辅助功能。

当前版本：`0.2.0-dev`（见 workspace `Cargo.toml`）。仓库：https://github.com/HadyLiu/IOT_Power

> 更完整的产品与设计文档在 [`Doc/`](Doc/README.md)。界面曾参考开源项目 [pd_pocket_pc_controller](https://github.com/ElluIFX/pd_pocket_pc_controller)；设备通信见 [协议文档](Doc/Communication/协议文档.md)。

## 功能概览

- **USB 直连**：通过 `nusb` 访问设备（VID `0x0408` / PID `0x1043`），Windows 走 WinUSB，Linux 需 udev 规则；同一时刻只能一个程序占用设备
- **实时监测**：电压 / 电流（自动量程）/ 功率，以及能量、平均功率、平均电流、负载阻值、电量 (mAh)
- **输出控制**：设定电压与限流（可配置安全边界）、ON/OFF、回显确认与失败回滚、急停；断线自动重连
- **曲线与录制**：双曲线、区间回看与统计、CSV / txt 采集日志、自动分文件、记录分析与回放
- **辅助功能**：预设组、序列执行、功率闭环、直流扫描、函数发生器、电池模拟（同一时刻只运行一个）
- **其它**：暗色 / 亮色与自定义主题、中英界面、悬浮窗、可选本机 HTTP 接口（默认关闭，需 token）
- **模拟模式**：`--sim` 可在无硬件时用内置模拟设备调试

## 仓库结构

| 路径 | 说明 |
|---|---|
| `app/` | 桌面程序 `iot-power`（egui/eframe UI） |
| `crates/dc3005u-proto/` | 协议编解码（无 IO） |
| `crates/dc3005u-usb/` | USB 传输与设备 IO 线程（含 `sim`） |
| `Doc/` | PRD、架构、协议、构建与用户手册等 |
| `packaging/` | udev 规则、桌面入口、图标、deb / Docker 等 |
| `scripts/` | `docker-build.sh` 等构建脚本 |

## 快速开始

### 依赖

- Rust（`rustup`；workspace 要求见 `Cargo.toml` 的 `rust-version`）
- Linux 另需：`build-essential`、`pkg-config`、`libudev-dev`，以及 egui 常用图形依赖（如 `libxkbcommon-dev`、`libgl1-mesa-dev`、`libwayland-dev`）

```bash
# Ubuntu 示例
sudo apt install -y build-essential pkg-config libudev-dev \
  libxkbcommon-dev libgl1-mesa-dev libwayland-dev
```

### 构建与运行

```bash
git clone https://github.com/HadyLiu/IOT_Power.git
cd IOT_Power

cargo run -p iot-power            # 调试运行（连接真实设备）
cargo run -p iot-power -- --sim   # 无硬件时用模拟设备
cargo build --release -p iot-power
```

常用参数：

| 参数 | 说明 |
|---|---|
| `--sim` | 使用内置模拟 DC3005U |
| `--english` | 强制英文界面 |
| `--debug` | 更详细日志（也可用 `RUST_LOG`） |
| `-h` / `--help` | 帮助 |

### Linux udev

首次连接设备前安装规则（`.deb` 安装时会自动放置）：

```bash
sudo cp packaging/99-dc3005u.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
# 重新插拔设备
```

使用前请关闭官方「IoT Power」等其它占用该 USB 接口的软件。

### 打包

本机 / Docker 打包流程见 [`Doc/构建发布流程.md`](Doc/构建发布流程.md)。常用入口：

```bash
bash scripts/docker-build.sh        # Linux + Windows 产物 → dist/
bash scripts/docker-build.sh check  # fmt + clippy + test 等
```

## 文档

| 文档 | 内容 |
|---|---|
| [Doc/README.md](Doc/README.md) | 文档总览与特性说明 |
| [Doc/用户手册.md](Doc/用户手册.md) | 终端用户说明 |
| [Doc/构建发布流程.md](Doc/构建发布流程.md) | 构建、打包、发布 |
| [Doc/Communication/协议文档.md](Doc/Communication/协议文档.md) | DC3005U USB 协议 |
| [Doc/CHANGELOG.md](Doc/CHANGELOG.md) | 版本变更 |

## 开源协议

本项目以 [MIT License](LICENSE) 发布。Copyright (c) 2026 Hady Liu。

依赖（如 egui/eframe）各自遵循其许可证；若发行包内置第三方字体，请按其许可证（例如 SIL OFL）一并附带说明。
