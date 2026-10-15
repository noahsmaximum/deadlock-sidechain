//! Game input: `[SIDECHAIN]{json}` lines printed by the Panorama hook, read from the engine's
//! netconsole (`-netconport`, live TCP) and/or console.log (`-condebug`, tailed). Both may run at
//! once; lines carry (session, seq) so whichever source delivers first wins.

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

pub const TAG: &str = "[SIDECHAIN]";

#[derive(Debug, Default, Deserialize)]
pub struct HookLine {
    pub s: Option<String>,
    pub n: Option<u64>,
    pub ready: Option<String>,
    pub hp: Option<f32>,
    pub hpm: Option<f32>,
    pub ammo: Option<f32>,
    pub ammom: Option<f32>,
    pub spd: Option<f32>,
    pub dead: Option<u8>,
    pub shop: Option<u8>,
    pub sb: Option<u8>,
    pub combat: Option<u8>,
    pub live: Option<u8>,
    pub mb: Option<String>,
    pub err: Option<String>,
    #[serde(default)]
    pub ev: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HookEvent {
    /// kill, assist, ks, hit, crit, dmgout
    Simple(String),
    Txt { source: String, main: String, sub: String, positive: Option<bool> },
    Ability { slot: u32, name: String },
    Engine(Vec<String>),
}

impl HookLine {
    pub fn events(&self) -> Vec<HookEvent> {
        self.ev.iter().filter_map(parse_event).collect()
    }
}

fn s(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(o) => o.to_string(),
    }
}

fn parse_event(v: &Value) -> Option<HookEvent> {
    match v {
        Value::String(name) => Some(HookEvent::Simple(name.clone())),
        Value::Array(a) => match a.first()?.as_str()? {
            "txt" => Some(HookEvent::Txt {
                source: s(a.get(1)),
                main: s(a.get(2)),
                sub: s(a.get(3)),
                positive: a.get(4).and_then(Value::as_i64).map(|p| p != 0),
            }),
            "ab" => Some(HookEvent::Ability { slot: a.get(1)?.as_u64()? as u32, name: s(a.get(2)) }),
            "ge" => Some(HookEvent::Engine(a.iter().skip(1).map(|x| s(Some(x))).collect())),
            other => Some(HookEvent::Simple(other.to_string())),
        },
        _ => None,
    }
}

/// Pull the JSON payload out of any console line carrying our tag.
pub fn parse_line(line: &str) -> Option<HookLine> {
    let i = line.find(TAG)?;
    serde_json::from_str(line[i + TAG.len()..].trim()).ok()
}

/// Drops lines already seen from the other source (same session, seq not newer).
#[derive(Default)]
pub struct Dedupe {
    session: String,
    last: u64,
}

impl Dedupe {
    pub fn accept(&mut self, l: &HookLine) -> bool {
        let (Some(s), Some(n)) = (&l.s, l.n) else { return true };
        if *s != self.session {
            // A new session only replaces the current one with its first lines (ready / n small),
            // so a stale tail of an old log can't hijack a live netcon session.
            if n > 50 && !self.session.is_empty() && l.ready.is_none() {
                return false;
            }
            self.session = s.clone();
            self.last = n;
            return true;
        }
        if n <= self.last {
            return false;
        }
        self.last = n;
        true
    }
}

#[derive(Default)]
pub struct InputStatus {
    pub netcon: AtomicBool,
    pub log: AtomicBool,
}

pub fn spawn_netcon(port: u16, tx: Sender<String>, status: Arc<InputStatus>) {
    if port == 0 {
        return;
    }
    std::thread::Builder::new()
        .name("netcon".into())
        .spawn(move || {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            let mut announced = false;
            loop {
                match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
                    Ok(stream) => {
                        let _ = stream.set_nodelay(true);
                        status.netcon.store(true, Ordering::Relaxed);
                        crate::log(format!("netcon: connected to :{port}"));
                        announced = false;
                        let mut r = BufReader::new(stream);
                        let mut buf = Vec::new();
                        loop {
                            buf.clear();
                            match r.read_until(b'\n', &mut buf) {
                                Ok(0) | Err(_) => break,
                                Ok(_) => {
                                    let line = String::from_utf8_lossy(&buf);
                                    if line.contains(TAG) && tx.send(line.into_owned()).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        status.netcon.store(false, Ordering::Relaxed);
                        crate::log("netcon: disconnected");
                    }
                    Err(_) => {
                        if !announced {
                            crate::log(format!("netcon: waiting for Deadlock on :{port} (launch option -netconport {port})"));
                            announced = true;
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        })
        .expect("spawn netcon");
}

pub fn spawn_log_tail(explicit: Option<PathBuf>, tx: Sender<String>, status: Arc<InputStatus>) {
    std::thread::Builder::new()
        .name("logtail".into())
        .spawn(move || {
            let mut waited = false;
            loop {
                let path = explicit.clone().or_else(find_console_log);
                match path.filter(|p| p.exists()) {
                    Some(p) => {
                        crate::log(format!("console.log: tailing {}", p.display()));
                        status.log.store(true, Ordering::Relaxed);
                        if tail(&p, &tx).is_err() {
                            return; // receiver gone
                        }
                        status.log.store(false, Ordering::Relaxed);
                    }
                    None if !waited => {
                        crate::log("console.log: not found yet (Deadlock not running, or no -condebug)");
                        waited = true;
                    }
                    None => {}
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        })
        .expect("spawn logtail");
}

/// Follow `path` from its current end. Returns Ok when the file disappears/shrinks (re-resolve),
/// Err when the channel is closed.
fn tail(path: &PathBuf, tx: &Sender<String>) -> Result<(), ()> {
    let Ok(mut f) = std::fs::File::open(path) else { return Ok(()) };
    let mut pos = f.seek(SeekFrom::End(0)).unwrap_or(0);
    let mut partial = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut idle = 0u32;
    loop {
        let len = match std::fs::metadata(path) {
            Ok(m) => m.len(),
            Err(_) => return Ok(()),
        };
        if len < pos {
            // truncated: a new game session started writing the log from scratch
            pos = 0;
            partial.clear();
            if f.seek(SeekFrom::Start(0)).is_err() {
                return Ok(());
            }
        }
        if len > pos {
            idle = 0;
            while let Ok(n) = f.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                pos += n as u64;
                partial.extend_from_slice(&chunk[..n]);
                while let Some(nl) = partial.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = partial.drain(..=nl).collect();
                    let line = String::from_utf8_lossy(&line);
                    if line.contains(TAG) {
                        tx.send(line.into_owned()).map_err(|_| ())?;
                    }
                }
            }
        } else {
            idle = idle.saturating_add(1);
        }
        // 10 ms while the log is moving, back off to 100 ms once it has been quiet for ~5 s
        std::thread::sleep(Duration::from_millis(if idle > 500 { 100 } else { 10 }));
    }
}

/// <Deadlock>\game\citadel\console.log, derived from the running deadlock.exe's image path.
pub fn find_console_log() -> Option<PathBuf> {
    let p = crate::sessions::processes().into_iter().find(|p| p.exe.eq_ignore_ascii_case("deadlock.exe"))?;
    let exe = crate::sessions::image_path(p.pid)?;
    // …\Deadlock\game\bin\win64\deadlock.exe -> …\Deadlock\game\citadel\console.log
    Some(exe.parent()?.parent()?.parent()?.join("citadel").join("console.log"))
}
