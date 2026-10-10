//! 中英文案。key 与 `Doc/详细设计.md` §6 一致；英文沿用原版 `en_US.ts` 译文。

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(l: Lang) {
    LANG.store(l as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    if LANG.load(Ordering::Relaxed) == Lang::En as u8 { Lang::En } else { Lang::Zh }
}

/// `language` 设置为 auto 时：系统语言是中文用中文，否则英文。
pub fn detect_system_lang() -> Lang {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return if v.to_ascii_lowercase().starts_with("zh") { Lang::Zh } else { Lang::En };
        }
    }
    // Windows 一般不设 LANG，默认中文
    Lang::Zh
}

/// 取文案；key 不存在时返回 key 本身（debug 构建下会断言）。
pub fn t(key: &'static str) -> &'static str {
    match TABLE.iter().find(|(k, ..)| *k == key) {
        Some((_, zh, en)) => match lang() {
            Lang::Zh => zh,
            Lang::En => en,
        },
        None => {
            debug_assert!(false, "missing i18n key {key}");
            key
        }
    }
}

/// 窗口标题前缀。Linux Wayland 下标题栏由 winit 的客户端装饰绘制，
/// 用的是系统默认字体、没有中文回退，中文会显示成方框，所以固定用英文。
pub fn window_title() -> &'static str {
    let wayland_csd = cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    if wayland_csd { t_in(Lang::En, "app_title") } else { t("app_title") }
}

fn t_in(l: Lang, key: &'static str) -> &'static str {
    match TABLE.iter().find(|(k, ..)| *k == key) {
        Some((_, zh, en)) => match l {
            Lang::Zh => zh,
            Lang::En => en,
        },
        None => key,
    }
}

/// 带占位符的文案：`tf("allowed_range", &[("min", "0.8"), ...])`
pub fn tf(key: &'static str, args: &[(&str, &str)]) -> String {
    let mut s = t(key).to_owned();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

#[rustfmt::skip]
static TABLE: &[(&str, &str, &str)] = &[
    ("common_ok", "确定", "OK"),
    ("common_cancel", "取消", "Cancel"),
    ("common_warning", "警告", "Warning"),
    ("common_error", "错误", "Error"),
    ("common_close", "关闭", "Close"),
    ("common_na", "N/A", "N/A"),
    ("common_retry", "重试", "Retry"),
    ("common_quit", "退出", "Quit"),
    ("app_title", "IOT_Power 数控电源上位机", "IOT_Power Digital Power Supply Controller"),
    ("state_disconnected", "未连接", "Disconnected"),
    ("state_connected", "已连接", "Connected"),
    ("state_connecting", "连接中…", "Connecting…"),
    ("state_waiting_device", "等待设备…", "Waiting for device…"),
    ("state_reconnecting", "重连中（第 {n} 次）", "Reconnecting ({n})"),
    ("state_error", "连接错误", "Connection error"),
    ("btn_connect", "连接", "LINK"),
    ("btn_disconnect", "断开", "UNLINK"),
    ("connect_failed", "连接失败", "Connect failed"),
    ("lcd_voltage", "电压", "VOUT"),
    ("lcd_current", "电流", "IOUT"),
    ("lcd_power", "功率", "POWER"),
    ("lcd_energy", "能量 (J)", "ENERGY (J)"),
    ("lcd_avg_power", "平均功率", "AVG-P"),
    ("lcd_load_r", "负载阻值", "LOAD-R"),
    ("btn_zero", "回零", "ZERO"),
    ("zero_tip", "清零平均功率和能量累计", "Reset the average power and energy accumulation"),
    ("output_title", "输出设定 / OUTPUT", "OUTPUT SETTING"),
    ("output_state", "输出状态", "State"),
    ("output_quickset", "快速设定", "Quick Set"),
    ("output_quickset_tip", "快速设定：当设定电压/电流变化时立刻提交", "Quick Setup: Submit immediately when V/I setting changes"),
    ("set_voltage", "设定电压", "Voltage"),
    ("set_current", "限流", "Current Limit"),
    ("output_on", "-  ON  -", "-  ON  -"),
    ("output_off", "-  OFF  -", "-  OFF  -"),
    ("output_unknown", "未知", "UNKNOWN"),
    ("confirm_output_on", "确定要打开输出?", "Sure you want to turn on the output?"),
    ("output_unconfirmed_hint", "状态未经设备确认", "State not confirmed by device"),
    ("output_acked_tip", "设备不回读输出状态，以最后一次已应答的命令为准", "The device does not report output state; showing the last acknowledged command"),
    ("setpoint_uncommitted", "未提交", "Not applied"),
    ("device_setpoint", "设备 {v} V / {i} A", "Device {v} V / {i} A"),
    ("device_setpoint_none", "设备 —", "Device —"),
    ("echo_changed_draft_kept", "设备设定已变为 {v} V / {i} A，你的编辑未提交", "Device setpoint changed to {v} V / {i} A, your edit is not applied"),
    ("device_setpoint_out_of_limits", "设备当前设定超出安全边界", "Device setpoint is outside safety limits"),
    ("btn_graphics", "图形设置", "GRAPHICS SET"),
    ("btn_settings", "电源设置", "DEVICE SET"),
    ("dlg_device_title", "电源设置", "Device Setting"),
    ("dev_section_conn", "- 连接设置 -", "- Connection -"),
    ("dev_section_other", "其它设置", "Other"),
    ("dev_output_warn", "输出开启警告", "Output Warning"),
    ("dev_output_warn_tip", "开启输出前显示警告", "Show warning before turning on output"),
    ("dev_lock_output", "输出时锁定参数", "Lock U/I when output"),
    ("dev_lock_output_tip", "输出状态下不允许包括辅助功能在内的设定值修改操作", "Volt and current changes is not allowed when output is on"),
    ("dev_poll_interval", "轮询间隔", "Poll Interval"),
    ("dev_timeout", "读写超时", "USB Timeout"),
    ("dev_resp_timeout", "应答超时", "Response Timeout"),
    ("dev_resp_timeout_tip", "命令 / 轮询", "command / poll"),
    ("dev_auto_connect", "启动时自动连接", "Connect on startup"),
    ("dev_off_on_connect", "首次连接时关闭输出", "Turn output off on first connect"),
    ("dev_identity", "型号 {model}  序列号 {serial}  固件 {fw}", "Model {model}  S/N {serial}  FW {fw}"),
    ("dev_cmd_stats", "命令统计：成功 {ok} / 超时 {to} / 无效 {bad}，平均 {rtt} ms", "Commands: ok {ok} / timeout {to} / invalid {bad}, avg {rtt} ms"),
    ("gfx_language", "界面语言", "Language"),
    ("gfx_theme", "界面主题", "UI Theme"),
    ("gfx_theme_dark", "暗黑风格", "Dark"),
    ("gfx_theme_light", "明亮风格", "Light"),
    ("dlg_graphics_title", "图形设置", "Graphic Settings"),
    ("gfx_interp", "插值方式", "Interpolation"),
    ("gfx_zero_threshold", "置零阈值", "Zero Threshold"),
    ("gfx_state_fps", "状态区刷新率", "State FPS"),
    ("current_unit", "电流单位", "Current Unit"),
    ("current_unit_auto", "自动", "Auto"),
    ("err_not_found", "未找到 DC3005U（VID 0408 / PID 1043），请检查线缆、驱动，或在设备管理器中启用设备", "DC3005U (VID 0408 / PID 1043) not found. Check the cable and driver, or enable the device in Device Manager"),
    ("err_busy", "设备被占用，请关闭官方 IoT Power 软件后重试", "Device is busy. Close the official IoT Power software and retry"),
    ("err_permission", "没有 USB 访问权限，请安装 udev 规则后重新插拔设备", "No USB permission. Install the udev rule and replug the device"),
    ("err_handshake", "设备没有应答握手命令（0x01）", "Device did not answer the handshake (0x01)"),
    ("hw_limits", "硬件极限：0.8~24 V，0.5~5.0 A", "Hardware limits: 0.8–24 V, 0.5–5.0 A"),
    ("allowed_range", "允许范围 {min}~{max} {unit}", "Allowed range {min}–{max} {unit}"),
    ("setpoint_out_of_limits", "{field} 超出安全边界（允许 {min}~{max} {unit}），未下发", "{field} outside safety limits (allowed {min}–{max} {unit}), not sent"),
    ("safety_v_min_hw", "电压下限须在 0.8~24 V 内", "Min voltage must be within 0.8–24 V"),
    ("safety_v_max_hw", "电压上限须在 0.8~24 V 内", "Max voltage must be within 0.8–24 V"),
    ("safety_i_min_hw", "电流下限须在 0.5~5.0 A 内", "Min current must be within 0.5–5.0 A"),
    ("safety_i_max_hw", "电流上限须在 0.5~5.0 A 内", "Max current must be within 0.5–5.0 A"),
    ("safety_v_order", "电压下限不能大于电压上限", "Min voltage must not exceed max voltage"),
    ("safety_i_order", "电流下限不能大于电流上限", "Min current must not exceed max current"),
    ("safety_not_number", "请输入数字", "Enter a number"),
    ("safety_setpoint_outside_new", "当前设定值超出新的安全边界，请调整", "Current setpoint is outside the new safety limits, please adjust"),
    ("safety_adjust_before_on", "请先把设定值调整到安全边界内", "Adjust the setpoint into the safety limits first"),
    ("safety_section", "- 安全边界 -", "- Safety Limits -"),
    ("safety_v_min", "电压下限", "Min Voltage"),
    ("safety_v_max", "电压上限", "Max Voltage"),
    ("safety_i_min", "电流下限", "Min Current"),
    ("safety_i_max", "电流上限", "Max Current"),
    ("safety_reset", "恢复硬件极限", "Reset to Hardware Limits"),
    ("safety_locked_output_on", "请先关闭输出再修改安全边界", "Turn the output off before changing safety limits"),
    ("safety_summary", "边界 {vmin}~{vmax} V / {imin}~{imax} A", "Limits {vmin}–{vmax} V / {imin}–{imax} A"),
    ("safety_summary_tip", "安全边界：所有设定值都必须在此范围内，超界拒绝下发。点击修改（输出关闭时）", "Safety limits: every setpoint must be inside this range or it is rejected. Click to edit (output off)"),
    ("safety_settings_reset", "安全边界设置无效，已恢复为硬件极限", "Invalid safety limits, reset to hardware limits"),
    ("presets_fixed", "部分预设超出硬件极限，已修正", "Some presets were outside hardware limits and have been corrected"),
    ("settings_load_failed", "读取设置失败，已使用默认设置：{err}", "Failed to read settings, using defaults: {err}"),
    ("settings_save_failed", "保存设置失败：{err}", "Failed to save settings: {err}"),
    ("locked_when_output", "输出时锁定参数已开启", "Parameters are locked while output is on"),
    ("exit_output_on", "输出仍为开，是否关闭输出后退出？", "Output is still on. Turn it off before exiting?"),
    ("exit_off_and_quit", "关闭输出并退出", "Turn off and quit"),
    ("exit_keep_and_quit", "保持输出退出", "Keep output and quit"),
    ("exit_disconnected", "设备未连接，输出可能仍为开，请检查设备后再退出", "Device not connected, output may still be on. Check the device before quitting"),
    ("exit_turning_off", "正在关闭输出…", "Turning output off…"),
    ("cmd_set_failed", "设定未生效：{reason}，已恢复为 {v} V / {i} A", "Setpoint not applied: {reason}, restored to {v} V / {i} A"),
    ("cmd_on_failed", "打开输出失败：{reason}", "Failed to turn output on: {reason}"),
    ("cmd_off_failed", "关闭输出失败，请手动关闭设备输出", "Failed to turn output off, please switch it off on the device"),
    ("cmd_timeout", "设备无应答", "No response from device"),
    ("cmd_bad_response", "设备应答无效", "Invalid response from device"),
    ("cmd_echo_mismatch", "设定回显不一致", "Setpoint echo mismatch"),
    ("cmd_not_connected", "设备未连接", "Device not connected"),
    ("cmd_usb_error", "USB 错误", "USB error"),
    ("on_setpoint_failed", "设定未生效，未打开输出", "Setpoint not applied, output not turned on"),
    ("link_lost_banner", "设备已断开，输出可能仍为开，请检查设备", "Device disconnected, output may still be on, please check the device"),
    ("pending_off_queued", "已记录关输出请求，重连后立即执行", "Output-off request queued, will be sent after reconnect"),
    ("estop_failed", "急停关输出失败，请立即手动关闭设备输出", "E-stop failed to turn output off, switch it off on the device now"),
    ("serial_changed", "已连接设备序列号 {sn} 与上次（{last}）不同", "Connected device S/N {sn} differs from last ({last})"),
    ("guard_triggered", "下发值被兜底钳位：{from} → {to}（请报告此问题）", "Setpoint clamped by the last-resort guard: {from} → {to} (please report this)"),
    ("cali_out_of_hw", "修正后下发值 {value} {unit} 超出硬件极限 {min}~{max} {unit}", "Corrected value {value} {unit} outside hardware limits {min}–{max} {unit}"),
    ("qty_voltage", "电压", "Voltage"),
    ("qty_current", "电流", "Current"),
    ("qty_power", "功率", "Power"),
    ("qty_resistance", "阻值", "Resistance"),
    ("qty_none", "无", "None"),
    ("graph_upper", "上图", "Upper"),
    ("graph_lower", "下图", "Lower"),
    ("btn_hold", "保持", "HOLD"),
    ("btn_clear", "清空", "CLEAR"),
    ("graph_full", "全览", "All"),
    ("graph_manual", "手动缩放", "Manual zoom"),
    ("graph_manual_tip", "允许拖动 / 滚轮缩放曲线，双击复位", "Drag / scroll to zoom the chart, double-click to reset"),
    ("graph_full_tip", "显示缓冲区全部数据（按 min/max 抽稀）", "Show the whole buffer (min/max decimated)"),
    ("graph_follow_tip", "跟随最新 {n} 点", "Follow the latest {n} points"),
    ("graph_no_data", "暂无数据，连接设备后开始绘制", "No data yet, connect a device to start plotting"),
    ("graph_time", "时间 (s)", "Time (s)"),
    ("buffer_usage", "缓冲 {pct}%", "Buffer {pct}%"),
    ("confirm_clear_buffer", "清空所有曲线数据？", "Clear all chart data?"),
    ("stat_line", "平均 {avg}  最大 {max}  最小 {min}  峰峰 {pp} {unit}", "avg {avg}  max {max}  min {min}  p-p {pp} {unit}"),
    ("sample_rate", "采样率 {hz} Hz", "Sample rate {hz} Hz"),
    ("sampling_note", "每次轮询只取一个读数，更短的电流脉冲可能漏掉，能量为估算值", "One reading per poll; shorter current pulses may be missed, energy is an estimate"),
    ("ui_dropped_samples", "界面丢弃 {n} 点", "{n} points dropped in UI"),
    ("sim_mode", "模拟设备", "Simulated device"),    ("act_freq", "执行频率", "Act Freq"),
    ("ana_avg", "平均", "Avg"),
    ("ana_bad_file", "无法识别的记录文件：{reason}", "Unrecognized log file: {reason}"),
    ("ana_capacity", "容量 {mah} mAh", "Capacity {mah} mAh"),
    ("ana_duration", "时长 {t}", "Duration {t}"),
    ("ana_energy", "能量 {wh} Wh", "Energy {wh} Wh"),
    ("ana_events", "事件", "Events"),
    ("ana_export", "导出区间 CSV", "Export Range CSV"),
    ("ana_gaps", "断线时段 {n} 段，共 {t}", "{n} link gaps, total {t}"),
    ("ana_load", "载入 txt", "Load txt"),
    ("ana_loading", "正在解析… {pct}%", "Parsing… {pct}%"),
    ("ana_max", "最大", "Max"),
    ("ana_min", "最小", "Min"),
    ("ana_open", "分析记录", "Analyze Log"),
    ("ana_pause", "暂停", "Pause"),
    ("ana_play", "回放", "Play"),
    ("ana_rms", "RMS", "RMS"),
    ("ana_speed", "速度", "Speed"),
    ("ana_stats", "区间统计", "Range Statistics"),
    ("ana_title", "记录分析", "Log Analysis"),
    ("ana_truncated", "最后一行不完整，已忽略", "Last line incomplete, ignored"),
    ("auto_tip", "切换数据波形是否自动适应窗口", "Toggle automatic fitting of the waveform to the window"),
    ("aux_freq_limited", "实际执行频率 {hz} Hz（受下发间隔限制）", "Effective rate {hz} Hz (limited by set interval)"),
    ("aux_illegal", "非法参数", "Illegal Parameters"),
    ("aux_off", "功能已关闭", "Disabled"),
    ("aux_on", "功能已开启", "Enabled"),
    ("aux_param_out_of_limits", "无法启动：{param} 超出安全边界（允许 {min}~{max} {unit}）", "Cannot start: {param} outside safety limits (allowed {min}–{max} {unit})"),
    ("aux_param_rejected", "{param} 超出安全边界，修改未生效", "{param} is outside safety limits, change ignored"),
    ("aux_read_stalled", "读数中断，功能已暂停", "Readings stalled, function paused"),
    ("aux_send_failed", "下发失败 {n} 次", "Set failed {n} times"),
    ("aux_stopped_send_failed", "连续下发失败，功能已停止", "Repeated set failures, function stopped"),
    ("aux_title", "辅助功能 / AUX FUNC", "AUXILIARY FUNCTION"),
    ("aux_wait_stable", "等待输出稳定…", "Waiting for output to settle…"),
    ("bat_capacity", "单节容量", "Capacity/S"),
    ("bat_cells", "多节串联", "Series"),
    ("bat_curve", "放电曲线", "DisChr Curve"),
    ("bat_discharge_time", "放电时长: {time}", "Discharge Time: {time}*"),
    ("bat_internal_r", "单节内阻", "InterRes/S"),
    ("bat_load_curve", "加载曲线", "Load Curve"),
    ("bat_over_vmax", "曲线最高电压 × 串联数超过电压上限 {v_max} V，请减少串联数", "Curve max voltage × cells exceeds the voltage limit {v_max} V, reduce cell count"),
    ("bat_soc_now", "当前电量", "Cur Level"),
    ("bat_soc_stop", "截止电量", "Stop Level"),
    ("bat_view_curve", "查看曲线", "View Curve"),
    ("btn_auto", "适应", "AUTO"),
    ("btn_dump", "导出", "DUMP"),
    ("btn_float", "浮窗", "FLOAT"),
    ("btn_manual", "手动", "MANUAL"),
    ("btn_record", "录制", "REC"),
    ("btn_record_stop", "停止", "STOP"),
    ("btn_release", "解除", "REL"),
    ("buffer_usage_tip", "数据缓冲区占用率", "Data buffer used"),
    ("clear_tip", "清空波形数据缓冲区", "Clear the waveform data buffer"),
    ("common_save", "保存", "Save"),
    ("csv_filter", "CSV文件 (*.csv)", "CSV files (*.csv)"),
    ("data_saved_to", "数据已保存至：", "Data has been saved to:"),
    ("display_range_tip", "显示范围", "Display range / pts"),
    ("dump_dialog", "保存数据", "Dump data to"),
    ("dump_done", "保存完成", "Dump success"),
    ("dump_tip", "导出当前数据缓冲区的有效数据", "Export valid data from data buffer"),
    ("estop", "急停", "E-STOP"),
    ("estop_done", "已急停，输出已关闭", "Emergency stopped, output turned off"),
    ("float_collapse", "折叠", "Collapse"),
    ("float_current", "电流 I", "Current"),
    ("float_expand", "展开", "Expand"),
    ("float_power", "功率 P", "Power"),
    ("float_tip", "切换数据监控悬浮窗", "Switch the data monitoring floating window"),
    ("float_voltage", "电压 U", "Voltage"),
    ("gfx_antialias", "曲线抗锯齿", "Antialias"),
    ("gfx_apply_correction", "应用修正参数 (y=kx+b)", "Apply Correction Params (y=kx+b)*"),
    ("gfx_data_pts", "记录点数", "Graph Buffer"),
    ("gfx_display_pts", "最小显示点数", "Minimal Range"),
    ("gfx_downsample", "曲线降采样", "Downsample"),
    ("gfx_input", "输入方式", "Num Adjust"),
    ("gfx_input_bit", "按位更改", "Position"),
    ("gfx_input_free", "自由更改", "Free"),
    ("gfx_interp_0", "不进行插值", "No"),
    ("gfx_interp_1", "一位插值", "1Bit"),
    ("gfx_interp_2", "二位插值", "2Bit"),
    ("gfx_interp_3", "三位插值", "3Bit"),
    ("gfx_max_fps", "最大刷新率", "Max FPS"),
    ("gfx_section_correction", "显示修正 (读/写)", "Display Correction (R/W)*"),
    ("gfx_section_graph", "图表设置", "Graph Settings"),
    ("gfx_section_state", "状态区设置", "State Display Settings"),
    ("gfx_section_ui", "UI 设置", "UI Settings"),
    ("graph1_tip", "上窗口数据", "Upper graph window"),
    ("graph2_tip", "下窗口数据", "Lower graph window"),
    ("graph_data", "显示数据:", "Data:"),
    ("hold_tip", "停止数据波形刷新（数据缓冲区仍在更新）", "Stop refreshing the data waveform (the data buffer continues to update)"),
    ("http_allow_output_on", "允许远程打开输出", "Allow remote output on"),
    ("kp_max_v", "电压上限", "Max Volt"),
    ("kp_param", "闭环参数", "PID K"),
    ("kp_target", "目标功率", "Target P"),
    ("log_auto_start", "连接后自动开始录制", "Start recording on connect"),
    ("log_record_csv", "同时保存 CSV", "Also save CSV"),
    ("log_split", "自动分文件：每 {min} 分钟或 {mb} MB", "Split files every {min} min or {mb} MB"),
    ("open_path", "打开文件路径", "Open Path"),
    ("preset_edit", "修改预设", "Edit"),
    ("preset_rejected", "预设 {n} 超出安全边界：{field} 允许 {min}~{max} {unit}", "Preset {n} outside safety limits: {field} allowed {min}–{max} {unit}"),
    ("preset_save_fail", "保存失败", "Save failed"),
    ("preset_save_ok", "保存成功", "Save OK"),
    ("preset_select", "选择预设", "Select Preset"),
    ("record_done", "录制完成", "Record finished"),
    ("record_tip", "将原始数据记录到CSV文件，不受图形缓冲区限制", "Record the raw data to a CSV file without being limited by the graphics buffer"),
    ("remote_active", "远程控制中", "Remote control active"),
    ("result_export", "导出数据", "Export Data"),
    ("result_fit_order", "多项式拟合次数:", "Polynomial Fitting Order:"),
    ("result_not_enough", "点数不足", "Not enough points"),
    ("runtime_clamped", "运行中已钳位 {n} 次（瞬时值超出安全边界）", "Clamped {n} times during run (value outside safety limits)"),
    ("safety_item_out_of_bounds", "⚠ 超出安全边界", "⚠ Outside safety limits"),
    ("sample_interval", "采样间隔:", "Sample:"),
    ("sample_rate_tip", "实时采样率", "Realtime sample rate"),
    ("seq_add_action", "添加动作", "Add Action"),
    ("seq_confirm_clear", "确定要清空序列吗？", "Clear the whole sequence?"),
    ("seq_current", "电流", "I-SET"),
    ("seq_delay", "延迟", "Delay"),
    ("seq_edit_action", "编辑动作", "Edit action"),
    ("seq_input_current", "请输入电流值:", "Input current-set:"),
    ("seq_input_delay", "请输入延时时间:", "Input delay time:"),
    ("seq_input_voltage", "请输入电压值(0=关断):", "Input voltage-set (0 is OFF):"),
    ("seq_input_wait", "请输入等待时间:", "Input wait time:"),
    ("seq_line_rejected", "第 {line} 行超出安全边界，序列已停止", "Line {line} outside safety limits, sequence stopped"),
    ("seq_line_skipped", "第 {line} 行超出安全边界，已跳过", "Line {line} outside safety limits, skipped"),
    ("seq_load", "载入", "Load"),
    ("seq_loop", "循环", "Loop"),
    ("seq_menu_clear", "清空", "Clear all"),
    ("seq_menu_delete", "删除", "Delete"),
    ("seq_menu_edit", "编辑", "Edit"),
    ("seq_oob_action", "序列超界时", "On out-of-limit line"),
    ("seq_oob_skip", "跳过", "Skip"),
    ("seq_oob_stop", "停止", "Stop"),
    ("seq_oob_warning", "有 {n} 行超出安全边界，执行到时将{action}", "{n} lines outside safety limits, will {action} when reached"),
    ("seq_single", "单次", "Once"),
    ("seq_stop", "停止", "Stop"),
    ("seq_validation_error", "数据验证错误:", "Data validation failed:"),
    ("seq_voltage", "电压", "V-SET"),
    ("seq_wait", "等待", "Wait"),
    ("seq_wait_format", "格式: 年-月-日 时:分:秒", "Format: YYYY-MM-DD HH:MM:SS"),
    ("sweep_delay", "延迟", "Delay"),
    ("sweep_empty", "扫描响应记录为空", "Scan response record is empty"),
    ("sweep_eta", "预计 {n} 点，约 {t}", "{n} points, about {t}"),
    ("sweep_no_record", "不记录", "Off"),
    ("sweep_response", "扫描响应", "Response"),
    ("sweep_result_title", "扫描响应结果曲线", "Scan Response Result Curve"),
    ("sweep_start", "起始点", "Start"),
    ("sweep_step", "步进值", "Step"),
    ("sweep_stop", "结束点", "End"),
    ("sweep_target", "目标参数", "Target"),
    ("sweep_view", "查看响应曲线", "View Response Curve"),
    ("tab_batsim", "电池模拟", "Battery Simulator"),
    ("tab_keep_power", "功率闭环", "Power Keep"),
    ("tab_preset", "预设组", "Presets"),
    ("tab_seq", "序列执行", "Sequence"),
    ("tab_sweep", "直流扫描", "DC Sweep"),
    ("tab_wavegen", "函数发生器", "Function Generator"),
    ("txt_filter", "文本文件 (*.txt)", "Text file (*.txt)"),
    ("wave_high", "高电平", "High"),
    ("wave_low", "低电平", "Low"),
    ("wave_noise", "噪音", "Noise"),
    ("wave_period", "周期", "Period"),
    ("wave_period_too_short", "周期过短，波形会严重失真", "Period too short, waveform will be heavily distorted"),
    ("wave_sawtooth", "锯齿波", "Sawtooth"),
    ("wave_sine", "正弦波", "Sine"),
    ("wave_square", "方波", "Square"),
    ("wave_triangle", "三角波", "Triangle"),
    ("wave_type", "波形类型", "Type"),
    ("ana_all", "全部", "All"),
    ("ana_newer_version", "文件由更新版本生成，部分内容可能无法识别", "File was written by a newer version, some content may be ignored"),
    ("aux_finished", "{name} 已完成", "{name} finished"),
    ("aux_locked", "辅助功能运行中，设定由功能控制", "Auxiliary function running, setpoints are controlled by it"),
    ("aux_other_running", "{name} 正在运行，请先停止", "{name} is running, stop it first"),
    ("aux_param_invalid", "参数无效：{param} 应在 {min}~{max} {unit}", "Invalid parameter: {param} must be {min}–{max} {unit}"),
    ("aux_poll_slow", "轮询间隔 {poll} ms 慢于执行频率 {hz} Hz，闭环会使用旧读数", "Poll interval {poll} ms is slower than the {hz} Hz rate, control will use stale readings"),
    ("aux_stopped_link", "设备断开，{name} 已停止", "Device disconnected, {name} stopped"),
    ("aux_will_turn_on", "启动「{name}」会打开输出", "Starting \"{name}\" will turn the output on"),
    ("bat_curve_bad", "曲线文件无效：{err}", "Invalid curve file: {err}"),
    ("bat_cutoff", "已到截止电量，电池模拟结束", "Stop level reached, battery simulation finished"),
    ("bat_initial_v", "起始电压", "Initial voltage"),
    ("bat_low_clamp", "满载时电池电压可能低于电压下限，届时将被钳位", "At full load the battery voltage may drop below the voltage limit and will be clamped"),
    ("click_for_details", "点击查看详情", "Click for details"),
    ("gfx_cali_read", "读数", "Reading"),
    ("gfx_cali_set", "设定", "Setpoint"),
    ("gfx_correction_tip", "读数按 y=k·x+b 显示；设定值反算后下发", "Readings are shown as y=k·x+b; setpoints are inverted before sending"),
    ("gfx_data_pts_tip", "缓冲区最多保留的点数（100~1000000），辅助功能运行中不可改", "Max points kept in the buffer (100–1000000), locked while an auxiliary function runs"),
    ("gfx_input_bit_tip", "输入框获得焦点后，↑↓ / 滚轮按光标所在位步进", "When an input is focused, ↑↓ / wheel step the digit at the cursor"),
    ("gfx_palette", "配色", "Palette"),
    ("gfx_palette_reset", "恢复默认配色", "Reset palette"),
    ("gfx_section_record", "录制设置", "Recording"),
    ("gfx_theme_delete", "删除主题", "Delete theme"),
    ("gfx_theme_new", "新主题名", "New theme name"),
    ("http_copy_token", "复制 token", "Copy token"),
    ("http_enabled", "启用本地 HTTP 接口", "Enable local HTTP API"),
    ("http_enabled_tip", "仅监听 127.0.0.1，需要 token 认证", "Listens on 127.0.0.1 only, token required"),
    ("http_new_token", "重新生成", "Regenerate"),
    ("http_port", "端口", "Port"),
    ("http_section", "远程接口", "Remote API"),
    ("http_start_failed", "HTTP 接口启动失败：{err}", "Failed to start HTTP API: {err}"),
    ("log_debug_columns", "记录调试列", "Debug columns"),
    ("log_dir", "保存目录", "Folder"),
    ("log_flush", "写盘间隔", "Flush interval"),
    ("log_split_label", "自动分文件（0 = 不分）", "Split files (0 = off)"),
    ("output_aux_estop_tip", "辅助功能运行中，点击即急停", "Auxiliary function running, click to emergency stop"),
    ("record_failed", "录制失败：{err}", "Recording failed: {err}"),
    ("recording", "录制中", "REC"),
    ("safety_locked_aux", "辅助功能运行中，安全边界不可修改", "Safety limits are locked while an auxiliary function runs"),
    ("seq_loop_no_delay", "循环执行需要至少一行延迟或等待", "Loop needs at least one delay or wait line"),
    ("sweep_same_ends", "起始点与结束点相同", "Start and end are equal"),
    ("wave_high_le_low", "高电平必须大于低电平", "High must be greater than low"),
    ("wave_triangle_tip", "一个波形周期的时长", "Duration of one waveform cycle"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_unique() {
        let mut keys: Vec<_> = TABLE.iter().map(|(k, ..)| *k).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n);
    }

    #[test]
    fn zh_en_share_placeholders() {
        let names = |s: &str| {
            let mut v: Vec<String> =
                s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|(n, _)| n.to_string())).collect();
            v.sort();
            v.dedup();
            v
        };
        for (k, zh, en) in TABLE {
            assert_eq!(names(zh), names(en), "placeholders differ for {k}");
        }
    }

    #[test]
    fn placeholders() {
        set_lang(Lang::Zh);
        assert_eq!(tf("allowed_range", &[("min", "0.8"), ("max", "24"), ("unit", "V")]), "允许范围 0.8~24 V");
    }
}
