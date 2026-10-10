#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod analysis;
mod app;
mod auxfn;
mod buffer;
mod cali;
mod config;
mod data;
mod fit;
mod format;
mod http;
mod i18n;
mod output;
mod setpoint;
mod theme;
mod ui;

use std::sync::OnceLock;
use std::time::Duration;

use crossbeam_channel::Sender;
use dc3005u_usb::{Cmd, Connector, NusbConnector, sim::SimDevice};
use eframe::egui;

use crate::config::Settings;
use crate::data::DataCmd;

static PANIC_DEV: OnceLock<Sender<Cmd>> = OnceLock::new();
static PANIC_DATA: OnceLock<Sender<DataCmd>> = OnceLock::new();

/// 崩溃时尽力关输出、把录制写盘，并在配置目录留 crash.log
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(tx) = PANIC_DEV.get() {
            let _ = tx.send(Cmd::EStop);
        }
        if let Some(tx) = PANIC_DATA.get() {
            let (done_tx, done_rx) = crossbeam_channel::bounded(1);
            if tx.send(DataCmd::Flush(done_tx)).is_ok() {
                let _ = done_rx.recv_timeout(Duration::from_millis(500));
            }
        }
        if let Some(dir) = Settings::path().as_deref().and_then(std::path::Path::parent) {
            let text = format!(
                "{} iot-power {} panic: {info}\n{}\n",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                env!("CARGO_PKG_VERSION"),
                std::backtrace::Backtrace::force_capture()
            );
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join("crash.log"), text);
        }
        default(info);
    }));
}

struct Args {
    sim: bool,
    english: bool,
    debug: bool,
}

fn parse_args() -> Args {
    let mut a = Args { sim: false, english: false, debug: false };
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--sim" => a.sim = true,
            "--english" => a.english = true,
            "--debug" => a.debug = true,
            "-h" | "--help" => {
                println!(
                    "iot-power {}\n\n  --sim      use a simulated DC3005U (no hardware)\n  --english  force English UI\n  --debug    verbose logging (or set RUST_LOG)",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    a
}

fn main() -> eframe::Result {
    let args = parse_args();
    let default_level = if args.debug { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level)).init();

    let (settings, notices) = Settings::load();
    if args.english {
        i18n::set_lang(i18n::Lang::En);
    } else {
        ui::graphics_dialog::apply_language(&settings.language);
    }

    let (sim, connector): (Option<SimDevice>, Box<dyn Connector>) = if args.sim {
        let s = SimDevice::new();
        let c = Box::new(s.connector());
        (Some(s), c)
    } else {
        (None, Box::new(NusbConnector))
    };

    let (io_tx, io_rx) = crossbeam_channel::unbounded();
    let dev = dc3005u_usb::spawn(connector, settings.worker_config(), io_tx);
    let _ = PANIC_DEV.set(dev.sender());
    install_panic_hook();

    let size = settings.window.map_or([920.0, 800.0], |w| [w.w.max(640.0), w.h.max(480.0)]);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(format!("{} v{}", i18n::window_title(), env!("CARGO_PKG_VERSION")))
        .with_inner_size(size)
        .with_min_inner_size([640.0, 480.0]);
    if let Some(w) = settings.window
        && let (Some(x), Some(y)) = (w.x, w.y)
    {
        viewport = viewport.with_position([x, y]);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    eframe::run_native(
        "IOT_Power",
        options,
        Box::new(move |cc| {
            theme::install_cjk_font(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            let data = data::spawn(io_rx, app::display_cfg(&settings), Box::new(move || ctx.request_repaint()));
            let _ = PANIC_DATA.set(data.cmd.clone());
            Ok(Box::new(app::App::new(&cc.egui_ctx, settings, notices, sim, dev, data)))
        }),
    )
}
