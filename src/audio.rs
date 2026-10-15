//! Capture the app's audio (process loopback, wherever it is routed) and play it through the DSP
//! chain on the output device. Capture runs on the sink's clock and render on the output's, so a
//! cubic resampler with a slow PI loop on buffer fill absorbs the drift.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use wasapi::{AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

use crate::config::{PARAMS, ParamValues, STEM_PARAMS};
use crate::dsp::Chain;
use crate::neural::NeuralStems;
use crate::stems::Stems;

/// The stems stage: fast DSP approximation or neural (HT-Demucs) separation.
pub enum StemProc {
    Dsp(Stems),
    Neural(NeuralStems, u32, bool),
}

impl StemProc {
    fn process(&mut self, buf: &mut [f32], db: [f32; 3]) {
        match self {
            StemProc::Dsp(s) => s.process(buf, db),
            StemProc::Neural(n, _, _) => n.process(buf, db),
        }
    }
    fn levels(&self) -> [f32; 3] {
        match self {
            StemProc::Dsp(s) => s.levels_db,
            StemProc::Neural(n, _, _) => n.levels_db,
        }
    }
}

/// Engine rate. 44.1 kHz = Spotify's source rate and HT-Demucs' rate; Windows converts at the devices.
pub const RATE: usize = 44_100;
const MAX_QUEUE_FRAMES: usize = RATE; // 1 s; anything beyond is dropped as stale

pub struct Shared {
    /// Interleaved stereo frames from capture, oldest first.
    pub ring: Mutex<VecDeque<f32>>,
    pub targets: Mutex<ParamValues>,
    pub bypass: AtomicBool,
    /// Process-tree root to capture; 0 = none.
    pub capture_pid: AtomicU32,
    /// MMDevice id to render to; empty = system default (followed live).
    pub output_id: Mutex<String>,
    pub latency_frames: AtomicU32,
    pub stop: AtomicBool,
    // stats
    pub underruns: AtomicU32,
    pub ratio_ppm: AtomicU64, // (ratio-1)*1e6 + 1e6, so it fits unsigned
    pub out_peak: AtomicU32,  // f32 bits
    pub capturing: AtomicBool,
    pub output_name: Mutex<String>,
    pub stems_enabled: AtomicBool,
    pub stems_neural: AtomicBool,
    pub neural_hop_ms: AtomicU32,
    pub neural_gpu: AtomicBool,
    pub neural_model: Mutex<std::path::PathBuf>,
    pub neural: Arc<crate::neural::NeuralStatus>,
    /// Current added latency of the stems stage, ms.
    pub stems_latency_ms: AtomicU32,
    /// Stem activity in dB (f32 bits): drums, vocals, music.
    pub stem_levels: [AtomicU32; 3],
}

impl Shared {
    pub fn new(latency_ms: f32) -> Arc<Self> {
        let mut t = [0.0; PARAMS.len()];
        for p in PARAMS {
            t[p.index()] = p.neutral();
        }
        Arc::new(Shared {
            ring: Mutex::new(VecDeque::with_capacity(MAX_QUEUE_FRAMES * 2)),
            targets: Mutex::new(t),
            bypass: AtomicBool::new(false),
            capture_pid: AtomicU32::new(0),
            output_id: Mutex::new(String::new()),
            latency_frames: AtomicU32::new((latency_ms.max(5.0) * RATE as f32 / 1000.0) as u32),
            stop: AtomicBool::new(false),
            underruns: AtomicU32::new(0),
            ratio_ppm: AtomicU64::new(1_000_000),
            out_peak: AtomicU32::new(0),
            capturing: AtomicBool::new(false),
            output_name: Mutex::new(String::new()),
            stems_enabled: AtomicBool::new(false),
            stems_neural: AtomicBool::new(false),
            neural_hop_ms: AtomicU32::new(2000),
            neural_gpu: AtomicBool::new(true),
            neural_model: Mutex::new(std::path::PathBuf::new()),
            neural: Arc::new(crate::neural::NeuralStatus::default()),
            stems_latency_ms: AtomicU32::new(0),
            stem_levels: [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)],
        })
    }
}

fn fmt() -> WaveFormat {
    WaveFormat::new(32, 32, &SampleType::Float, RATE, 2, None)
}

// ------------------------------------------------------------------------------------- capture

pub fn spawn_capture(sh: Arc<Shared>) {
    std::thread::Builder::new()
        .name("capture".into())
        .spawn(move || {
            let _ = wasapi::initialize_mta();
            while !sh.stop.load(Ordering::Relaxed) {
                let pid = sh.capture_pid.load(Ordering::Relaxed);
                if pid == 0 {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
                if let Err(e) = capture(&sh, pid) {
                    crate::log(format!("capture: {e}"));
                    std::thread::sleep(Duration::from_secs(1));
                }
                sh.capturing.store(false, Ordering::Relaxed);
            }
        })
        .expect("spawn capture");
}

fn capture(sh: &Shared, pid: u32) -> Result<()> {
    let mut client = AudioClient::new_application_loopback_client(pid, true)?;
    client.initialize_client(&fmt(), &Direction::Capture, &StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 })?;
    let ev = client.set_get_eventhandle()?;
    let cc = client.get_audiocaptureclient()?;
    let mut bytes: VecDeque<u8> = VecDeque::new();
    client.start_stream()?;
    sh.capturing.store(true, Ordering::Relaxed);
    crate::log(format!("capture: listening to pid {pid} (process loopback)"));
    while !sh.stop.load(Ordering::Relaxed) && sh.capture_pid.load(Ordering::Relaxed) == pid {
        if ev.wait_for_event(100).is_err() {
            continue; // nothing playing
        }
        while cc.get_next_packet_size()?.unwrap_or(0) > 0 {
            let before = bytes.len();
            let info = cc.read_from_device_to_deque(&mut bytes)?;
            if info.flags.silent {
                for b in bytes.range_mut(before..) {
                    *b = 0;
                }
            }
        }
        if bytes.len() >= 8 {
            let (a, b) = bytes.as_slices();
            let mut flat = Vec::with_capacity(bytes.len());
            flat.extend_from_slice(a);
            flat.extend_from_slice(b);
            let whole = flat.len() / 8 * 8;
            let mut ring = sh.ring.lock().unwrap();
            ring.extend(flat[..whole].chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])));
            let excess = ring.len().saturating_sub(MAX_QUEUE_FRAMES * 2);
            ring.drain(..excess);
            drop(ring);
            bytes.drain(..whole);
        }
    }
    client.stop_stream()?;
    Ok(())
}

// ------------------------------------------------------------------------------------- resampler

/// Pulls frames from the shared queue at `ratio` input frames per output frame, steering ratio so
/// the queue hovers at the latency target.
pub struct Resampler {
    buf: Vec<[f32; 2]>,
    pos: f64,
    priming: bool,
    fill_lp: f64,
    integ: f64,
    pub ratio: f64,
}

impl Resampler {
    pub fn new() -> Self {
        Resampler { buf: Vec::new(), pos: 1.0, priming: true, fill_lp: 0.0, integ: 0.0, ratio: 1.0 }
    }

    /// Fill `out` (interleaved stereo). Returns false on underrun (rest zero-filled).
    pub fn produce(&mut self, queue: &Mutex<VecDeque<f32>>, out: &mut [f32], target: f64) -> bool {
        {
            let mut q = queue.lock().unwrap();
            let n = q.len() / 2;
            self.buf.reserve(n);
            let mut it = q.drain(..n * 2);
            while let (Some(l), Some(r)) = (it.next(), it.next()) {
                self.buf.push([l, r]);
            }
        }
        let avail = self.buf.len() as f64 - self.pos;
        let frames = out.len() / 2;
        if self.priming {
            if avail < target {
                out.fill(0.0);
                return true;
            }
            // start `target` behind the newest frame
            self.pos = (self.buf.len() as f64 - target).max(1.0);
            self.fill_lp = target;
            self.integ = 0.0;
            self.priming = false;
        } else if avail > target * 4.0 + frames as f64 {
            // stalled render or a burst: jump back to target instead of slowly chewing through it
            self.pos = self.buf.len() as f64 - target;
        }

        let dt = frames as f64 / RATE as f64;
        self.fill_lp += (avail - self.fill_lp) * (dt / 2.0).min(1.0); // ~2 s smoothing
        let err = (self.fill_lp - target) / target;
        self.integ = (self.integ + err * dt).clamp(-5.0, 5.0);
        self.ratio = 1.0 + (0.002 * err + 0.0004 * self.integ).clamp(-0.005, 0.005);

        let mut ok = true;
        for (k, fr) in out.chunks_exact_mut(2).enumerate() {
            let i = self.pos.floor() as usize;
            if i + 2 >= self.buf.len() || i == 0 {
                for x in out[k * 2..].iter_mut() {
                    *x = 0.0;
                }
                self.priming = true;
                ok = false;
                break;
            }
            let t = (self.pos - i as f64) as f32;
            for c in 0..2 {
                fr[c] = hermite(self.buf[i - 1][c], self.buf[i][c], self.buf[i + 1][c], self.buf[i + 2][c], t);
            }
            self.pos += self.ratio;
        }
        // keep one frame of history behind the read head
        let drop = (self.pos.floor() as usize).saturating_sub(1).min(self.buf.len());
        self.buf.drain(..drop);
        self.pos -= drop as f64;
        if self.priming {
            self.buf.clear();
            self.pos = 1.0;
        }
        ok
    }
}

#[inline]
fn hermite(xm1: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    let c = (x1 - xm1) * 0.5;
    let v = x0 - x1;
    let w = c + v;
    let a = w + v + (x2 - x0) * 0.5;
    let b = w + a;
    (((a * t) - b) * t + c) * t + x0
}

// ------------------------------------------------------------------------------------- render

pub fn spawn_render(sh: Arc<Shared>) {
    std::thread::Builder::new()
        .name("render".into())
        .spawn(move || {
            let _ = wasapi::initialize_mta();
            let mut chain = Chain::new(RATE as f32);
            let mut rs = Resampler::new();
            let mut stems: Option<StemProc> = None;
            while !sh.stop.load(Ordering::Relaxed) {
                if let Err(e) = render(&sh, &mut chain, &mut rs, &mut stems) {
                    crate::log(format!("render: {e}"));
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        })
        .expect("spawn render");
}

fn resolve_output(en: &DeviceEnumerator, id: &str) -> Result<wasapi::Device> {
    if id.is_empty() {
        Ok(en.get_default_device(&Direction::Render)?)
    } else {
        en.get_device(id).map_err(|e| anyhow!("output device {id}: {e}"))
    }
}

fn render(sh: &Shared, chain: &mut Chain, rs: &mut Resampler, stems: &mut Option<StemProc>) -> Result<()> {
    let en = DeviceEnumerator::new()?;
    let want = sh.output_id.lock().unwrap().clone();
    let dev = resolve_output(&en, &want)?;
    let dev_id = dev.get_id()?;
    let name = dev.get_friendlyname().unwrap_or_default();
    let mut client = dev.get_iaudioclient()?;
    let (def, _) = client.get_device_period()?;
    client.initialize_client(&fmt(), &Direction::Render, &StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: def })?;
    let ev = client.set_get_eventhandle()?;
    let rc = client.get_audiorenderclient()?;
    let silence = vec![0u8; client.get_available_space_in_frames()? as usize * 8];
    rc.write_to_device(silence.len() / 8, &silence, None)?;
    client.start_stream()?;
    crate::log(format!("render: playing on {name}"));
    *sh.output_name.lock().unwrap() = name;

    let mut f32buf: Vec<f32> = Vec::new();
    let mut bytes: Vec<u8> = Vec::new();
    let mut checked = Instant::now();
    let mut targets = *sh.targets.lock().unwrap();
    let mut peak = 0f32;
    while !sh.stop.load(Ordering::Relaxed) {
        ev.wait_for_event(1000).map_err(|_| anyhow!("output stopped responding"))?;
        let n = client.get_available_space_in_frames()? as usize;
        if n == 0 {
            continue;
        }
        f32buf.resize(n * 2, 0.0);
        let target = sh.latency_frames.load(Ordering::Relaxed) as f64;
        if !rs.produce(&sh.ring, &mut f32buf, target) {
            sh.underruns.fetch_add(1, Ordering::Relaxed);
        }
        if let Ok(t) = sh.targets.try_lock() {
            targets = *t;
        }
        chain.bypass = sh.bypass.load(Ordering::Relaxed);
        let want_neural = sh.stems_neural.load(Ordering::Relaxed);
        let hop_ms = sh.neural_hop_ms.load(Ordering::Relaxed);
        let gpu = sh.neural_gpu.load(Ordering::Relaxed);
        let rebuild = match (sh.stems_enabled.load(Ordering::Relaxed), stems.as_ref()) {
            (false, Some(_)) => {
                *stems = None;
                sh.stems_latency_ms.store(0, Ordering::Relaxed);
                false
            }
            (true, None) => true,
            (true, Some(StemProc::Dsp(_))) => want_neural,
            (true, Some(StemProc::Neural(_, h, g))) => !want_neural || *h != hop_ms || *g != gpu,
            (false, None) => false,
        };
        if rebuild {
            if want_neural {
                let model = sh.neural_model.lock().unwrap().clone();
                let hop = hop_ms as f32 / 1000.0;
                sh.neural.misses.store(0, Ordering::Relaxed);
                sh.neural.coverage.store(0, Ordering::Relaxed);
                *stems = Some(StemProc::Neural(NeuralStems::new(&model, hop, false, gpu, sh.neural.clone()), hop_ms, gpu));
                sh.stems_latency_ms.store((NeuralStems::latency_s(hop) * 1000.0) as u32, Ordering::Relaxed);
            } else {
                *stems = Some(StemProc::Dsp(Stems::new(RATE as f32)));
                sh.stems_latency_ms.store((crate::stems::N as f32 / RATE as f32 * 1000.0) as u32, Ordering::Relaxed);
            }
        }
        if let Some(st) = stems.as_mut() {
            // bypass keeps the stems in the path at unity so latency doesn't jump on A/B
            let db = if chain.bypass { [0.0; 3] } else { STEM_PARAMS.map(|p| targets[p.index()]) };
            st.process(&mut f32buf, db);
            let lv = st.levels();
            for i in 0..3 {
                sh.stem_levels[i].store(lv[i].to_bits(), Ordering::Relaxed);
            }
        }
        chain.process(&mut f32buf, &targets);
        for s in &f32buf {
            peak = peak.max(s.abs());
        }
        bytes.clear();
        bytes.extend(f32buf.iter().flat_map(|s| s.to_le_bytes()));
        rc.write_to_device(n, &bytes, None)?;

        if checked.elapsed() > Duration::from_millis(250) {
            checked = Instant::now();
            sh.ratio_ppm.store(((rs.ratio - 1.0) * 1e6 + 1e6) as u64, Ordering::Relaxed);
            sh.out_peak.store(peak.to_bits(), Ordering::Relaxed);
            peak = 0.0;
            // re-open when the wanted device changes (or the system default moves)
            let want_now = sh.output_id.lock().unwrap().clone();
            let moved = if want_now.is_empty() {
                en.get_default_device(&Direction::Render).and_then(|d| d.get_id()).map(|id| id != dev_id).unwrap_or(false)
            } else {
                want_now != dev_id
            };
            if moved {
                break;
            }
        }
    }
    client.stop_stream()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Capture clock 150 ppm fast, delivered in 10 ms bursts with jitter; render pulls 10 ms
    /// periods. After priming there must be no underruns, the loop must lock to the drift, and a
    /// 1 kHz sine must come out without discontinuities.
    #[test]
    fn resampler_tracks_drift() {
        let q = Mutex::new(VecDeque::new());
        let mut rs = Resampler::new();
        let target = RATE as f64 * 0.030; // 30 ms
        let cap_rate = RATE as f64 * (1.0 + 150e-6);
        let (mut cap_t, mut cap_phase, mut cap_due) = (0.0f64, 0.0f64, 0.0f64);
        let mut out = vec![0f32; RATE / 100 * 2]; // 10 ms periods
        let mut underruns = 0;
        let mut worst_jump = 0f32;
        let mut prev: Option<f32> = None;
        for period in 0..(60 * 100) {
            let now = (period + 1) as f64 * 0.010;
            // capture side: bursts every ~10 ms (+-3 ms jitter) on its own clock
            while cap_t < now {
                let jitter = ((period * 7919) % 7) as f64 * 0.001 - 0.003;
                cap_t += 0.010 + jitter * 0.1;
                cap_due += 0.010 * cap_rate;
                let n = cap_due.floor() as usize;
                cap_due -= n as f64;
                let mut g = q.lock().unwrap();
                for _ in 0..n {
                    let s = (cap_phase * 2.0 * std::f64::consts::PI).sin() as f32 * 0.5;
                    cap_phase += 1000.0 / cap_rate;
                    g.push_back(s);
                    g.push_back(s);
                }
            }
            if !rs.produce(&q, &mut out, target) && period > 10 {
                underruns += 1;
            }
            if period > 200 {
                for fr in out.chunks_exact(2) {
                    if let Some(p) = prev {
                        worst_jump = worst_jump.max((fr[0] - p).abs());
                    }
                    prev = Some(fr[0]);
                }
            }
        }
        let ppm = (rs.ratio - 1.0) * 1e6;
        let fill = q.lock().unwrap().len() / 2;
        println!("ratio {ppm:+.1} ppm, queue {fill} frames, underruns {underruns}, worst step {worst_jump:.4}");
        assert_eq!(underruns, 0);
        assert!((ppm - 150.0).abs() < 40.0, "ratio didn't lock to drift: {ppm}");
        // a 1 kHz sine at 0.5 amplitude moves at most 2*pi*1000/RATE*0.5 per sample
        let max_step = 2.0 * std::f32::consts::PI * 1000.0 / RATE as f32 * 0.5;
        assert!(worst_jump < max_step * 1.1, "discontinuity {worst_jump} (max {max_step})");
    }
}
