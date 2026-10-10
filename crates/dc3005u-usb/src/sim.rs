//! 模拟 DC3005U：按协议应答 `0x01/0x03/0x05/0x06`，带电阻负载与限流（CC）模型。
//! 用于 IO 线程单元测试，以及没有实机时 `iot-power --sim` 调界面。
//! 测试可以通过 [`SimState`] 的开关注入故障（无应答、回显不跟随、帧序号冻结、拔线等）。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use dc3005u_proto::{cmd, synth_identity, synth_measurement_frame};

use crate::transport::{Connector, OpenError, Transport, TransportError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoKind {
    Write,
    Read,
    ReadTimeout,
}

#[derive(Debug, Clone)]
pub struct SimState {
    pub present: bool,
    pub busy: bool,
    pub serial: String,
    pub set_mv: u16,
    pub set_ma: u16,
    pub output_on: bool,
    pub load_ohm: f64,
    pub seq: u32,
    /// 帧序号不再递增（模拟设备帧更新慢 / 卡死）
    pub freeze_seq: bool,
    /// `0x03` 不应答
    pub drop_set_ack: bool,
    /// `0x06` 不应答
    pub drop_output_ack: bool,
    /// `0x05` 不应答
    pub drop_poll: bool,
    /// 回显是否跟随 `0x03`（false = 模拟设定未生效）
    pub echo_follows_set: bool,
    /// 下一次读时先返回的残留数据（模拟上一事务迟到的应答）
    pub stray: Option<Vec<u8>>,
    /// 每次拔线递增，旧连接随之失效
    pub generation: u64,
    /// 所有写入的帧
    pub writes: Vec<Vec<u8>>,
    /// IO 顺序记录
    pub io_log: Vec<IoKind>,
}

impl Default for SimState {
    fn default() -> Self {
        Self {
            present: true,
            busy: false,
            serial: "8B63A8F401E4".into(),
            set_mv: 5000,
            set_ma: 2000,
            output_on: false,
            load_ohm: 10.0,
            seq: 1000,
            freeze_seq: false,
            drop_set_ack: false,
            drop_output_ack: false,
            drop_poll: false,
            echo_follows_set: true,
            stray: None,
            generation: 0,
            writes: Vec::new(),
            io_log: Vec::new(),
        }
    }
}

#[derive(Clone, Default)]
pub struct SimDevice(Arc<Mutex<SimState>>);

impl SimDevice {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> MutexGuard<'_, SimState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn connector(&self) -> SimConnector {
        SimConnector(self.clone())
    }

    pub fn unplug(&self) {
        let mut s = self.state();
        s.present = false;
        s.generation += 1;
    }

    pub fn plug(&self) {
        self.state().present = true;
    }

    /// 写入记录中命令字为 `c` 的帧
    pub fn writes_of(&self, c: u8) -> Vec<Vec<u8>> {
        self.state().writes.iter().filter(|f| f.get(1) == Some(&c)).cloned().collect()
    }
}

pub struct SimConnector(SimDevice);

impl Connector for SimConnector {
    fn open(&mut self) -> Result<Box<dyn Transport>, OpenError> {
        let s = self.0.state();
        if !s.present {
            return Err(OpenError::NotFound);
        }
        if s.busy {
            return Err(OpenError::Busy);
        }
        let generation = s.generation;
        drop(s);
        Ok(Box::new(SimTransport { dev: self.0.clone(), generation, inbox: VecDeque::new() }))
    }
}

struct SimTransport {
    dev: SimDevice,
    generation: u64,
    inbox: VecDeque<Vec<u8>>,
}

impl SimTransport {
    fn alive(&self, s: &SimState) -> bool {
        s.present && s.generation == self.generation
    }
}

/// 输出端的电压/电流（V, A）：电阻负载 + 限流
fn output_point(s: &SimState) -> (f64, f64) {
    if !s.output_on {
        return (0.0, 0.0);
    }
    let v_set = f64::from(s.set_mv) / 1000.0;
    let i_lim = f64::from(s.set_ma) / 1000.0;
    let i = v_set / s.load_ohm.max(1e-6);
    if i > i_lim { (i_lim * s.load_ohm, i_lim) } else { (v_set, i) }
}

fn respond(s: &mut SimState, frame: &[u8]) -> Option<Vec<u8>> {
    let ok = frame.len() >= 8 && frame[0] == 0x55 && frame[frame.len() - 5] == 0xAA && {
        let (p, c) = frame.split_at(frame.len() - 4);
        crc32fast_hash(p).to_le_bytes() == c
    };
    if !ok {
        return None;
    }
    match frame[1] {
        cmd::CONNECT => Some(synth_identity("DC3005U", &s.serial.clone(), "1.9.5")),
        cmd::POLL => {
            if s.drop_poll {
                return None;
            }
            if !s.freeze_seq {
                s.seq = s.seq.wrapping_add(1);
            }
            let (v, i) = output_point(s);
            let v_mv = (v * 1000.0).round() as u16;
            let i_raw = (i * 1000.0 * 100_000.0).round() as u32;
            let p_raw = (v * i * 1e8).round().min(f64::from(u32::MAX)) as u32;
            Some(synth_measurement_frame(10072, s.seq, v_mv, i_raw, p_raw, s.set_mv, s.set_ma))
        }
        cmd::SET_VI if frame.len() >= 12 => {
            if s.drop_set_ack {
                return None;
            }
            if s.echo_follows_set {
                s.set_mv = u16::from_le_bytes([frame[3], frame[4]]);
                s.set_ma = u16::from_le_bytes([frame[5], frame[6]]);
            }
            Some(vec![cmd::SET_VI, 0x00])
        }
        cmd::OUTPUT if frame.len() >= 12 => {
            if s.drop_output_ack {
                return None;
            }
            s.output_on = frame[3] != 0;
            Some(vec![cmd::OUTPUT, 0x00])
        }
        _ => None,
    }
}

fn crc32fast_hash(p: &[u8]) -> u32 {
    // 与协议层一致的 CRC-32/IEEE；sim 不依赖 crc32fast，自己算一遍也顺便交叉校验
    let mut crc = 0xFFFF_FFFFu32;
    for &b in p {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

impl Transport for SimTransport {
    fn write(&mut self, data: &[u8], _timeout: Duration) -> Result<(), TransportError> {
        let mut s = self.dev.state();
        if !self.alive(&s) {
            return Err(TransportError::Disconnected);
        }
        s.writes.push(data.to_vec());
        s.io_log.push(IoKind::Write);
        if let Some(r) = respond(&mut s, data) {
            self.inbox.push_back(r);
        }
        Ok(())
    }

    fn read(&mut self, max_len: usize, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let mut s = self.dev.state();
        if !self.alive(&s) {
            return Err(TransportError::Disconnected);
        }
        let next = s.stray.take().or_else(|| self.inbox.pop_front());
        match next {
            Some(mut d) => {
                d.truncate(max_len);
                s.io_log.push(IoKind::Read);
                Ok(d)
            }
            None => {
                s.io_log.push(IoKind::ReadTimeout);
                drop(s);
                std::thread::sleep(timeout);
                Err(TransportError::Timeout)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_crc_matches_protocol() {
        let f = dc3005u_proto::frame_poll();
        let (p, c) = f.split_at(f.len() - 4);
        assert_eq!(crc32fast_hash(p).to_le_bytes(), c);
    }

    #[test]
    fn current_limit_model() {
        let s = SimState { output_on: true, set_mv: 10000, set_ma: 500, load_ohm: 10.0, ..Default::default() };
        let (v, i) = output_point(&s);
        assert!((i - 0.5).abs() < 1e-9 && (v - 5.0).abs() < 1e-9);
    }
}
