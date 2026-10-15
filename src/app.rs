//! Keeps the music app pointed at the sink while we run, and puts it back afterwards — including
//! when we die badly: a detached guardian process waits on us and restores the app's routing
//! (lingering until the app is running again if it has to).

use std::collections::HashSet;
use std::process::Child;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use wasapi::{DeviceEnumerator, Direction};

use crate::audio::Shared;
use crate::routing::{self, PolicyConfig};
use crate::sessions;

pub struct Router {
    exe: String,
    sink_id: String,
    pc: PolicyConfig,
    root: Option<u32>,
    routed: HashSet<u32>,
    /// The app's own override before we touched it (raw path); None = it followed the default.
    pub prior: Option<String>,
    prior_known: bool,
    guardian: Option<Child>,
    explicit_output: bool,
    sink_name: String,
    routed_at: Option<Instant>,
    /// Human-readable routing state for the UI, and whether it's the good state.
    pub state: String,
    pub ok: bool,
    last_logged: String,
}

pub fn find_render(name_part: &str) -> Result<wasapi::Device> {
    let en = DeviceEnumerator::new()?;
    let coll = en.get_device_collection(&Direction::Render)?;
    for i in 0..coll.get_nbr_devices()? {
        let d = coll.get_device_at_index(i)?;
        if d.get_friendlyname()?.to_lowercase().contains(&name_part.to_lowercase()) {
            return Ok(d);
        }
    }
    Err(anyhow!("no output device matching {name_part:?} (run `sidechain devices`)"))
}

impl Router {
    pub fn new(exe: &str, sink: &str, output: &str, sh: &Shared) -> Result<Self> {
        let sink_dev = find_render(sink)
            .map_err(|_| anyhow!("sink device {sink:?} not found. Any output you never listen to works (Steam Streaming Speakers, VB-Cable, an unused HDMI out); set [audio] sink in sidechain.toml"))?;
        let sink_id = sink_dev.get_id()?;
        let sink_name = sink_dev.get_friendlyname().unwrap_or_else(|_| sink.to_string());
        if !output.is_empty() {
            let id = find_render(output)?.get_id()?;
            if id == sink_id {
                return Err(anyhow!("output and sink are the same device"));
            }
            *sh.output_id.lock().unwrap() = id;
        }
        Ok(Router {
            exe: exe.to_string(),
            sink_id,
            pc: PolicyConfig::new()?,
            root: None,
            routed: HashSet::new(),
            prior: None,
            prior_known: false,
            guardian: None,
            explicit_output: !output.is_empty(),
            sink_name,
            routed_at: None,
            state: "waiting".into(),
            ok: false,
            last_logged: String::new(),
        })
    }

    /// Once a second: follow the app's process tree, route new instances to the sink.
    pub fn poll(&mut self, sh: &Shared) {
        let Some((root, pids)) = sessions::find_tree(&self.exe) else {
            if self.root.take().is_some() {
                crate::log(format!("{} closed", self.exe));
                sh.capture_pid.store(0, Ordering::Relaxed);
            }
            self.routed.clear();
            self.set_state(format!("{} not running", self.exe), false);
            return;
        };
        if self.root != Some(root) {
            crate::log(format!("{} found (pid {root})", self.exe));
            self.root = Some(root);
            sh.capture_pid.store(root, Ordering::Relaxed);
            self.routed.retain(|p| pids.contains(p));
        }
        let mut newly = 0;
        for &pid in &pids {
            if self.routed.contains(&pid) {
                continue;
            }
            // Only processes that own an audio session can be read/routed; others error.
            let Ok(cur) = self.pc.render_device(pid) else { continue };
            if !self.prior_known {
                let ours = cur.as_deref().and_then(routing::mmdevice_id).is_some_and(|id| id == self.sink_id);
                self.prior = if ours { None } else { cur };
                self.prior_known = true;
                if !self.explicit_output {
                    // play where the app was playing before (its own override), else the default
                    *sh.output_id.lock().unwrap() = self.prior.as_deref().and_then(routing::mmdevice_id).unwrap_or_default();
                }
            }
            if self.pc.set_render_device(pid, Some(&self.sink_id)).is_ok() {
                self.routed.insert(pid);
                newly += 1;
            }
        }
        if newly > 0 {
            crate::log(format!("{}: routed {newly} process(es) to the sink", self.exe));
            self.routed_at = Some(Instant::now());
            if self.guardian.is_none() {
                self.guardian = spawn_guardian(&self.exe, self.prior.as_deref());
            }
        }
        if self.routed.is_empty() {
            self.set_state(format!("{} found — press play once so it has an audio session", self.exe), false);
        } else if self.routed_at.is_some_and(|t| t.elapsed() > Duration::from_millis(1500)) {
            self.verify(&pids);
        } else {
            self.set_state("re-pointing to the sink…".into(), true);
        }
    }

    /// Where is the app actually playing? Only the sink = good; anywhere else = heard twice.
    fn verify(&mut self, pids: &HashSet<u32>) {
        let devs = active_session_devices(pids);
        let elsewhere: Vec<&String> = devs.iter().filter(|(id, _)| *id != self.sink_id).map(|(_, n)| n).collect();
        if devs.is_empty() {
            self.set_state(format!("{} paused (routed to {})", self.exe, self.sink_name), true);
        } else if elsewhere.is_empty() {
            self.set_state(format!("{} → {} → Sidechain ✓", self.exe, self.sink_name), true);
        } else {
            let names: Vec<&str> = elsewhere.iter().map(|s| s.as_str()).collect();
            self.set_state(
                format!("{} is ALSO still playing on {} (you'd hear it twice) — pause/play or restart it", self.exe, names.join(", ")),
                false,
            );
        }
    }

    fn set_state(&mut self, s: String, ok: bool) {
        if s != self.last_logged {
            if !s.starts_with("re-pointing") {
                crate::log(&s);
            }
            self.last_logged = s.clone();
        }
        self.state = s;
        self.ok = ok;
    }

    /// Give the app back but stay ready to take it again (game closed). Spotify is untouched
    /// while Deadlock isn't running.
    pub fn release(&mut self, sh: &Shared) {
        if self.prior_known || !self.routed.is_empty() {
            if self.restore() {
                crate::log(format!("{}: handed back (game not running)", self.exe));
            }
        }
        self.routed.clear();
        self.prior_known = false;
        self.prior = None;
        self.root = None;
        self.routed_at = None;
        sh.capture_pid.store(0, Ordering::Relaxed);
        self.set_state("Deadlock not running — Spotify untouched".into(), true);
        if !self.explicit_output {
            sh.output_id.lock().unwrap().clear();
        }
    }

    pub fn active(&self) -> bool {
        !self.routed.is_empty()
    }


    /// Put the app back. True if it could be done now (or nothing was ever changed).
    pub fn restore(&mut self) -> bool {
        let changed = self.prior_known;
        let ok = !changed || restore_app(&self.pc, &self.exe, self.prior.as_deref());
        if let Some(mut g) = self.guardian.take() {
            if ok {
                let _ = g.kill();
            } else {
                crate::log(format!("{} isn't running; a background helper will restore its output when it starts", self.exe));
            }
        }
        ok
    }
}

/// (device id, name) of every render device holding an *active* session for one of `pids`.
pub fn active_session_devices(pids: &HashSet<u32>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(en) = DeviceEnumerator::new() else { return out };
    let Ok(coll) = en.get_device_collection(&Direction::Render) else { return out };
    for i in 0..coll.get_nbr_devices().unwrap_or(0) {
        let Ok(d) = coll.get_device_at_index(i) else { continue };
        let Ok(mgr) = d.get_iaudiosessionmanager() else { continue };
        let Ok(list) = mgr.get_audiosessionenumerator() else { continue };
        for j in 0..list.get_count().unwrap_or(0) {
            let Ok(s) = list.get_session(j) else { continue };
            let active = matches!(s.get_state(), Ok(wasapi::SessionState::Active));
            if active && s.get_process_id().is_ok_and(|p| pids.contains(&p)) {
                out.push((d.get_id().unwrap_or_default(), d.get_friendlyname().unwrap_or_default()));
                break;
            }
        }
    }
    out
}

/// Set every instance of `exe` back to `prior`. True if at least one accepted it.
pub fn restore_app(pc: &PolicyConfig, exe: &str, prior: Option<&str>) -> bool {
    let Some((_, pids)) = sessions::find_tree(exe) else { return false };
    let mut ok = false;
    for pid in pids {
        ok |= pc.set_render_device_raw(pid, prior).is_ok();
    }
    ok
}

fn spawn_guardian(exe: &str, prior: Option<&str>) -> Option<Child> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x8;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
    let me = std::env::current_exe().ok()?;
    std::process::Command::new(me)
        .args(["__guardian", &std::process::id().to_string(), exe, prior.unwrap_or("")])
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .ok()
}

/// `sidechain __guardian <parent pid> <exe> <prior path or "">`
pub fn guardian(parent: u32, exe: &str, prior: &str) -> Result<()> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, parent) {
            WaitForSingleObject(h, INFINITE);
            let _ = CloseHandle(h);
        }
    }
    wasapi::initialize_mta().ok()?;
    let pc = PolicyConfig::new()?;
    let prior = (!prior.is_empty()).then_some(prior);
    // up to a day: the app may only be restarted much later
    for _ in 0..(24 * 3600) {
        if restore_app(&pc, exe, prior) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Ok(())
}
