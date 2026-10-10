//! 本地 HTTP 接口（F14，P2，`Doc/架构设计.md` §7）：只绑定 127.0.0.1，必须带 token，校验 Host，
//! 拒绝带 Origin 的请求（浏览器跨站），不发 CORS 头。
//! 读接口直接读数据线程快照；写接口转给 UI 线程，和界面操作走同一套校验（安全边界、锁定、互斥）。

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, bounded};
use dc3005u_usb::LinkState;
use serde_json::{Value, json};

use crate::data::{RECENT_CAP, SharedSnapshot};

const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY: u64 = 4096;

#[derive(Debug, Clone)]
pub struct HttpReply {
    pub status: u16,
    pub body: Value,
}

impl HttpReply {
    pub fn ok(body: Value) -> Self {
        Self { status: 200, body }
    }

    pub fn err(status: u16, msg: &str) -> Self {
        Self { status, body: json!({ "error": msg }) }
    }
}

/// 写请求，由 UI 线程处理
pub enum HttpReq {
    Setpoint { v: Option<f64>, i: Option<f64>, reply: Sender<HttpReply> },
    Output { on: bool, reply: Sender<HttpReply> },
}

/// UI 线程每帧发布的状态（输出可信度、设备设定等只有 UI 线程知道）
#[derive(Debug, Clone, Default)]
pub struct UiView {
    pub output: &'static str,
    pub vset: Option<f64>,
    pub iset: Option<f64>,
    pub aux: Option<&'static str>,
    pub safety: [f64; 4],
}

pub type SharedView = Arc<RwLock<UiView>>;

pub struct HttpServer {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

struct Ctx {
    port: u16,
    token: String,
    snap: SharedSnapshot,
    view: SharedView,
    req: Sender<HttpReq>,
    repaint: Box<dyn Fn() + Send>,
}

pub fn start(
    port: u16,
    token: String,
    snap: SharedSnapshot,
    view: SharedView,
    req: Sender<HttpReq>,
    repaint: Box<dyn Fn() + Send>,
) -> Result<HttpServer, String> {
    let server = tiny_http::Server::http(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    let ctx = Ctx { port, token, snap, view, req, repaint };
    let join = std::thread::Builder::new()
        .name("http".into())
        .spawn(move || {
            log::info!("HTTP API listening on 127.0.0.1:{port}");
            while !st.load(Ordering::Relaxed) {
                match server.recv_timeout(Duration::from_millis(200)) {
                    Ok(Some(rq)) => handle(&ctx, rq),
                    Ok(None) => {}
                    Err(e) => {
                        log::warn!("http: {e}");
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(HttpServer { stop, join: Some(join) })
}

fn header<'a>(rq: &'a tiny_http::Request, name: &str) -> Option<&'a str> {
    rq.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

fn eq_ct(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn respond(rq: tiny_http::Request, r: HttpReply) {
    let body = r.body.to_string();
    let ct = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("static header");
    let resp = tiny_http::Response::from_string(body).with_status_code(r.status).with_header(ct);
    let _ = rq.respond(resp);
}

fn handle(ctx: &Ctx, mut rq: tiny_http::Request) {
    let host_ok = header(&rq, "Host")
        .is_some_and(|h| h == format!("127.0.0.1:{}", ctx.port) || h == format!("localhost:{}", ctx.port));
    if !host_ok || header(&rq, "Origin").is_some() {
        return respond(rq, HttpReply::err(403, "forbidden host/origin"));
    }
    let token = header(&rq, "Authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| header(&rq, "X-Token"))
        .unwrap_or("");
    if ctx.token.len() < 16 || !eq_ct(token.trim(), &ctx.token) {
        return respond(rq, HttpReply::err(401, "missing or bad token"));
    }
    let url = rq.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let method = rq.method().clone();
    let reply = match (method, path) {
        (tiny_http::Method::Get, "/api/status") => HttpReply::ok(status_json(ctx)),
        (tiny_http::Method::Get, "/api/samples") => {
            let n = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("n="))
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(100)
                .min(RECENT_CAP);
            HttpReply::ok(samples_json(ctx, n))
        }
        (tiny_http::Method::Post, "/api/setpoint" | "/api/output") => {
            let mut body = String::new();
            if rq.as_reader().take(MAX_BODY).read_to_string(&mut body).is_err() {
                return respond(rq, HttpReply::err(400, "bad body"));
            }
            let Ok(v) = serde_json::from_str::<Value>(&body) else {
                return respond(rq, HttpReply::err(400, "body must be JSON"));
            };
            write_req(ctx, path, &v)
        }
        _ => HttpReply::err(404, "not found"),
    };
    respond(rq, reply);
}

fn write_req(ctx: &Ctx, path: &str, v: &Value) -> HttpReply {
    let (tx, rx) = bounded(1);
    let req = if path == "/api/setpoint" {
        let num = |k: &str| v.get(k).and_then(Value::as_f64);
        let (vv, ii) = (num("voltage"), num("current"));
        if vv.is_none() && ii.is_none() {
            return HttpReply::err(400, "need voltage and/or current");
        }
        HttpReq::Setpoint { v: vv, i: ii, reply: tx }
    } else {
        let Some(on) = v.get("on").and_then(Value::as_bool) else { return HttpReply::err(400, "need {\"on\": bool}") };
        HttpReq::Output { on, reply: tx }
    };
    if ctx.req.send(req).is_err() {
        return HttpReply::err(503, "app is shutting down");
    }
    (ctx.repaint)();
    rx.recv_timeout(REPLY_TIMEOUT).unwrap_or_else(|_| HttpReply::err(504, "no confirmation from device"))
}

fn link_str(l: LinkState) -> &'static str {
    match l {
        LinkState::Disconnected => "disconnected",
        LinkState::Connecting { .. } => "connecting",
        LinkState::Reconnecting { .. } => "reconnecting",
        LinkState::Connected { .. } => "connected",
    }
}

fn status_json(ctx: &Ctx) -> Value {
    let s = ctx.snap.read().expect("snapshot");
    let v = ctx.view.read().expect("view").clone();
    let reading = s.latest.map(|p| {
        json!({
            "voltage": p.v,
            "current": p.i_a,
            "power": p.v * p.i_a,
            "seq": p.sample.meas.seq,
            "age_ms": p.sample.t.elapsed().as_millis() as u64,
        })
    });
    json!({
        "link": link_str(s.link),
        "output": v.output,
        "setpoint": { "voltage": v.vset, "current": v.iset },
        "reading": reading,
        "energy_j": s.latest.map(|p| p.energy_j),
        "aux": v.aux,
        "recording": s.recording.as_ref().map(|r| r.0.display().to_string()),
        "safety": { "v_min": v.safety[0], "v_max": v.safety[1], "i_min": v.safety[2], "i_max": v.safety[3] },
    })
}

fn samples_json(ctx: &Ctx, n: usize) -> Value {
    let s = ctx.snap.read().expect("snapshot");
    let now = Instant::now();
    let skip = s.recent.len().saturating_sub(n);
    let arr: Vec<Value> = s
        .recent
        .iter()
        .skip(skip)
        .map(|p| {
            let age = now.saturating_duration_since(p.sample.t).as_secs_f64();
            json!({ "t": -age, "voltage": p.v, "current": p.i_a, "seq": p.sample.meas.seq })
        })
        .collect();
    json!({ "samples": arr })
}
