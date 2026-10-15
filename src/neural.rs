//! Neural stems: HT-Demucs v4 (forward-only ONNX export) on the GPU through DirectML.
//!
//! The export (huggingface.co/webnn/stem-separator, MIT) contains only the network; the
//! spectrogram front/back end is reproduced here exactly as demucs' `HTDemucs._spec` /
//! `_magnitude` / `_mask` / `_ispec` do it (torch.stft semantics: periodic Hann, centre +
//! reflect padding, normalized). `neural-check` verifies this against the full-graph export.
//!
//! Streaming: every `hop` seconds the last 7.8 s window is separated; we keep the slice that
//! ends `RIGHT` seconds before the window end (so every kept sample had future context), with
//! short crossfades between slices. Output = delayed mix + sum((g_i - 1) * stem_i), so with all
//! stem gains at 0 dB the output is the input exactly, just delayed.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Result, anyhow};
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

pub const SR: usize = 44_100;
pub const W: usize = 343_980; // 7.8 s training segment
const NFFT: usize = 4096;
const HL: usize = 1024;
const FR: usize = 2048; // freq bins kept (nfft/2, last dropped)
const T: usize = 336; // ceil(W / HL)
const PAD: usize = HL / 2 * 3; // 1536
pub const SOURCES: [&str; 4] = ["drums", "bass", "other", "vocals"];

/// Spectrogram front/back end + the ONNX session.
pub struct Demucs {
    session: ort::session::Session,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    win: Vec<f32>,
    pub device: String,
}

/// ONNX Runtime allows one environment per process.
pub fn env() -> Result<&'static ort::environment::Environment> {
    static ENV: std::sync::OnceLock<ort::environment::Environment> = std::sync::OnceLock::new();
    if let Some(e) = ENV.get() {
        return Ok(e);
    }
    let e = ort::init().with_name("sidechain").build()?;
    Ok(ENV.get_or_init(|| e))
}

fn reflect(i: isize, n: usize) -> usize {
    // numpy/torch 'reflect' (edge sample not repeated)
    let n = n as isize;
    let mut i = i;
    while i < 0 || i >= n {
        if i < 0 {
            i = -i;
        }
        if i >= n {
            i = 2 * (n - 1) - i;
        }
    }
    i as usize
}

fn mean_std(v: &[f32]) -> (f32, f32) {
    let n = v.len() as f64;
    let mean = v.iter().map(|&x| x as f64).sum::<f64>() / n;
    let var = v.iter().map(|&x| (x as f64 - mean).powi(2)).sum::<f64>() / (n - 1.0); // torch: unbiased
    (mean as f32, var.sqrt() as f32)
}

impl Demucs {
    /// `gpu`: try DirectML first (falls back to CPU if unavailable).
    pub fn load(model: &Path, gpu: bool, cpu_threads: usize) -> Result<Self> {
        use ort::session::Session;
        use ort::session::builder::GraphOptimizationLevel;
        let e = |e: ort::Error<_>| anyhow!("{e}");
        let env = env()?;
        let mk = |dml: bool| -> Result<Session> {
            let lvl = match std::env::var("SIDECHAIN_OPT").as_deref() {
                Ok("0") => GraphOptimizationLevel::Disable,
                Ok("1") => GraphOptimizationLevel::Level1,
                Ok("2") => GraphOptimizationLevel::Level2,
                _ => GraphOptimizationLevel::Level3,
            };
            let mut b = Session::builder(env)?.with_optimization_level(lvl).map_err(e)?;
            if dml {
                b = b
                    // DirectML's whole-graph fusion miscomputes this model (stems come out at +90 dB);
                    // unfused it matches the CPU result exactly (neural-sum) at ~0.43 s per window.
                    .with_config_entry("ep.dml.disable_graph_fusion", "1")
                    .map_err(e)?
                    .with_memory_pattern(false)
                    .map_err(e)?
                    .with_parallel_execution(false)
                    .map_err(e)?
                    .with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()])
                    .map_err(e)?;
            } else {
                b = b.with_intra_threads(cpu_threads).map_err(e)?;
            }
            Ok(b.commit_from_file(model)?)
        };
        let (session, device) = if gpu {
            match mk(true) {
                Ok(s) => (s, "GPU (DirectML)".to_string()),
                Err(err) => (mk(false)?, format!("CPU (DirectML failed: {err})")),
            }
        } else {
            (mk(false)?, format!("CPU ({cpu_threads} threads)"))
        };
        let mut planner = RealFftPlanner::<f32>::new();
        let win = (0..NFFT).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / NFFT as f32).cos()).collect();
        Ok(Demucs { session, r2c: planner.plan_fft_forward(NFFT), c2r: planner.plan_fft_inverse(NFFT), win, device })
    }

    /// HTDemucs._spec for one channel: complex [FR][T] (row-major f-major), torch-normalized.
    fn spec(&self, x: &[f32], out: &mut [Complex32]) {
        // demucs re-pad: reflect (PAD, PAD + T*HL - W) -> length L1
        let l1 = W + PAD + PAD + T * HL - W;
        let padded: Vec<f32> = (0..l1).map(|i| x[reflect(i as isize - PAD as isize, W)]).collect();
        // torch.stft center=True: reflect pad NFFT/2 on both sides; frames 2..2+T kept
        let half = NFFT / 2;
        let mut buf = vec![0f32; NFFT];
        let mut spec = self.r2c.make_output_vec();
        let mut scratch = self.r2c.make_scratch_vec();
        let norm = 1.0 / (NFFT as f32).sqrt();
        for t in 0..T {
            let frame = t + 2;
            for n in 0..NFFT {
                let i = (frame * HL + n) as isize - half as isize;
                buf[n] = padded[reflect(i, l1)] * self.win[n];
            }
            let _ = self.r2c.process_with_scratch(&mut buf, &mut spec, &mut scratch);
            for f in 0..FR {
                out[f * T + t] = spec[f] * norm;
            }
        }
    }

    /// HTDemucs._ispec for one channel: complex [FR][T] -> W samples.
    fn ispec(&self, z: &[Complex32], out: &mut [f32]) {
        // pad freq to NFFT/2+1 (zero) and 2 empty frames each side -> T+4 frames
        let frames = T + 4;
        let le = HL * T + 2 * PAD; // istft length requested by demucs
        let half = NFFT / 2;
        let total = le + 2 * half; // centre padding is removed after overlap-add
        let mut acc = vec![0f32; total + NFFT];
        let mut env = vec![0f32; total + NFFT];
        let mut spec = self.c2r.make_input_vec();
        let mut buf = vec![0f32; NFFT];
        let mut scratch = self.c2r.make_scratch_vec();
        // torch.istft normalized: frame = irfft(z) (1/N) * sqrt(N); realfft c2r has no 1/N
        let scale = 1.0 / (NFFT as f32).sqrt();
        for fr in 0..frames {
            let has = (2..2 + T).contains(&fr);
            for f in 0..=half {
                spec[f] = if has && f < FR { z[f * T + (fr - 2)] } else { Complex32::new(0.0, 0.0) };
            }
            spec[0].im = 0.0;
            spec[half].im = 0.0;
            let _ = self.c2r.process_with_scratch(&mut spec, &mut buf, &mut scratch);
            let off = fr * HL;
            for n in 0..NFFT {
                acc[off + n] += buf[n] * scale * self.win[n];
                env[off + n] += self.win[n] * self.win[n];
            }
        }
        // drop torch centre padding, then demucs' PAD
        for i in 0..W {
            let j = half + PAD + i;
            out[i] = if env[j] > 1e-11 { acc[j] / env[j] } else { 0.0 };
        }
    }

    /// Separate one stereo window (planar L then R, W samples each).
    /// Returns planar stems [s][c][n] (4 x 2 x W).
    pub fn separate(&mut self, mix: &[f32]) -> Result<Vec<f32>> {
        assert_eq!(mix.len(), 2 * W);
        // spectrogram (CAC: channels L.re L.im R.re R.im)
        let mut z = vec![vec![Complex32::new(0.0, 0.0); FR * T]; 2];
        for c in 0..2 {
            self.spec(&mix[c * W..(c + 1) * W], &mut z[c]);
        }
        let mut x = vec![0f32; 4 * FR * T];
        for c in 0..2 {
            for i in 0..FR * T {
                x[(c * 2) * FR * T + i] = z[c][i].re;
                x[(c * 2 + 1) * FR * T + i] = z[c][i].im;
            }
        }
        let (mean, std) = mean_std(&x);
        for v in x.iter_mut() {
            *v = (*v - mean) / (1e-5 + std);
        }
        let (meant, stdt) = mean_std(mix);
        let xt: Vec<f32> = mix.iter().map(|v| (v - meant) / (1e-5 + stdt)).collect();

        let xin = ort::value::Tensor::from_array(([1usize, 4, FR, T], x.into_boxed_slice()))?;
        let xtin = ort::value::Tensor::from_array(([1usize, 2, W], xt.into_boxed_slice()))?;
        let out = self.session.run(ort::inputs!["x" => xin, "xt" => xtin])?;
        let xo: Vec<f32> = out["x_out"].try_extract_tensor::<f32>()?.1.to_vec();
        let xto: Vec<f32> = out["xt_out"].try_extract_tensor::<f32>()?.1.to_vec();
        drop(out);

        let mut stems = vec![0f32; 4 * 2 * W];
        let mut zc = vec![Complex32::new(0.0, 0.0); FR * T];
        let mut wav = vec![0f32; W];
        for s in 0..4 {
            for c in 0..2 {
                // x_out channel layout: s*4 + c*2 + (re|im)
                let re = &xo[((s * 4 + c * 2) * FR * T)..((s * 4 + c * 2 + 1) * FR * T)];
                let im = &xo[((s * 4 + c * 2 + 1) * FR * T)..((s * 4 + c * 2 + 2) * FR * T)];
                for i in 0..FR * T {
                    zc[i] = Complex32::new(re[i] * std + mean, im[i] * std + mean);
                }
                self.ispec(&zc, &mut wav);
                let t = &xto[((s * 2 + c) * W)..((s * 2 + c + 1) * W)];
                let dst = &mut stems[(s * 2 + c) * W..(s * 2 + c + 1) * W];
                for i in 0..W {
                    dst[i] = t[i] * stdt + meant + wav[i];
                }
            }
        }
        Ok(stems)
    }
}

// ------------------------------------------------------------------------------- streaming

/// Shared status for the UI.
#[derive(Default)]
pub struct NeuralStatus {
    pub ready: AtomicBool,
    pub infer_ms: AtomicU32,
    pub misses: AtomicU64,
    pub device: Mutex<String>,
    pub error: Mutex<String>,
}

struct Job {
    end: u64,
    audio: Vec<f32>,
}
struct Done {
    end: u64,
    stems: Vec<f32>,
}

#[derive(Clone, Copy, Default)]
struct Acc {
    s: [[f32; 2]; 4],
    w: f32,
}

pub struct NeuralStems {
    hop: usize,
    right: usize,
    xf: usize,
    delay: usize,
    blocking: bool,
    hist: VecDeque<[f32; 2]>,
    in_count: u64,
    out_count: u64,
    next_job: u64,
    pending: usize,
    jobs: Sender<Job>,
    done: Receiver<Done>,
    delay_line: VecDeque<[f32; 2]>,
    store: VecDeque<Acc>,
    store_base: u64,
    gains: [f32; 4],
    zero_run: usize,
    fade: usize,
    pub status: Arc<NeuralStatus>,
    pub levels_db: [f32; 3],
    lvl_acc: [f32; 3],
    lvl_n: usize,
}

/// Seconds of future context every kept sample has.
pub const RIGHT_S: f32 = 1.0;
const XFADE_S: f32 = 0.25;

impl NeuralStems {
    /// Total added latency for a hop, in seconds.
    pub fn latency_s(hop_s: f32) -> f32 {
        RIGHT_S + hop_s + XFADE_S + 0.6
    }

    pub fn new(model: &Path, hop_s: f32, blocking: bool, status: Arc<NeuralStatus>) -> Self {
        let hop = (hop_s.clamp(0.5, 5.0) * SR as f32) as usize;
        let right = (RIGHT_S * SR as f32) as usize;
        let xf = (XFADE_S * SR as f32) as usize;
        let delay = (Self::latency_s(hop as f32 / SR as f32) * SR as f32) as usize;
        let (jtx, jrx) = channel::<Job>();
        let (dtx, drx) = channel::<Done>();
        let st = status.clone();
        let model = model.to_path_buf();
        std::thread::Builder::new()
            .name("neural".into())
            .spawn(move || worker(model, jrx, dtx, st, !blocking))
            .expect("spawn neural");
        status.ready.store(false, Ordering::Relaxed);
        NeuralStems {
            hop,
            right,
            xf,
            delay,
            blocking,
            hist: VecDeque::with_capacity(W + 4096),
            in_count: 0,
            out_count: 0,
            next_job: hop as u64,
            pending: 0,
            jobs: jtx,
            done: drx,
            delay_line: VecDeque::with_capacity(delay + 4096),
            store: VecDeque::new(),
            store_base: 0,
            gains: [1.0; 4],
            zero_run: 0,
            fade: 0,
            status,
            levels_db: [-90.0; 3],
            lvl_acc: [0.0; 3],
            lvl_n: 0,
        }
    }

    fn take_results(&mut self, wait: bool) {
        loop {
            let r = if wait && self.pending > 0 { self.done.recv().ok() } else { self.done.try_recv().ok() };
            let Some(d) = r else { return };
            self.pending = self.pending.saturating_sub(1);
            self.accumulate(d);
            if wait {
                return;
            }
        }
    }

    fn accumulate(&mut self, d: Done) {
        // window covers [end - W, end); keep [end - right - hop - xf, end - right)
        let start = d.end as i64 - W as i64;
        let a = d.end as i64 - (self.right + self.hop + self.xf) as i64;
        let b = d.end as i64 - self.right as i64;
        if self.store.is_empty() {
            self.store_base = a.max(0) as u64;
        }
        let len = (b - a) as usize;
        for j in 0..len {
            let abs = a + j as i64;
            if abs < self.store_base as i64 || abs < start {
                continue;
            }
            let w = if j < self.xf {
                j as f32 / self.xf as f32
            } else if j >= len - self.xf {
                (len - j) as f32 / self.xf as f32
            } else {
                1.0
            };
            let idx = (abs as u64 - self.store_base) as usize;
            while self.store.len() <= idx {
                self.store.push_back(Acc::default());
            }
            let off = (abs - start) as usize;
            let acc = &mut self.store[idx];
            for s in 0..4 {
                for c in 0..2 {
                    acc.s[s][c] += w * d.stems[(s * 2 + c) * W + off];
                }
            }
            acc.w += w;
        }
    }

    /// Process interleaved stereo in place. `target_db` = drums, vocals, music gains.
    pub fn process(&mut self, buf: &mut [f32], target_db: [f32; 3]) {
        let g = target_db.map(|d| 10f32.powf(d.max(-60.0) / 20.0));
        // stems: drums, bass, other, vocals  <-  drums, music, music, vocals
        let want = [g[0], g[2], g[2], g[1]];
        self.take_results(false);
        for fr in buf.chunks_exact_mut(2) {
            let x = [fr[0], fr[1]];
            // input side
            self.hist.push_back(x);
            if self.hist.len() > W {
                self.hist.pop_front();
            }
            self.in_count += 1;
            self.zero_run = if x[0] == 0.0 && x[1] == 0.0 { self.zero_run + 1 } else { 0 };
            if self.in_count >= self.next_job {
                self.next_job += self.hop as u64;
                let mut audio = vec![0f32; 2 * W];
                let pad = W - self.hist.len();
                for (i, f) in self.hist.iter().enumerate() {
                    audio[pad + i] = f[0];
                    audio[W + pad + i] = f[1];
                }
                if self.jobs.send(Job { end: self.in_count, audio }).is_ok() {
                    self.pending += 1;
                }
            }
            // a pause shouldn't play out the ~4 s still queued: fade and drop it
            if self.zero_run == SR / 4 && self.delay_line.iter().any(|f| f[0] != 0.0 || f[1] != 0.0) {
                self.fade = SR / 50;
            }
            self.delay_line.push_back(x);
            if self.delay_line.len() <= self.delay {
                fr[0] = 0.0;
                fr[1] = 0.0;
                continue;
            }
            // output side
            let mut y = self.delay_line.pop_front().unwrap();
            let o = self.out_count;
            self.out_count += 1;
            for i in 0..4 {
                self.gains[i] += (want[i] - self.gains[i]) * 0.0005;
            }
            if self.blocking {
                while self.pending > 0 && (self.store.is_empty() || o >= self.store_base + self.store.len() as u64) {
                    self.take_results(true);
                }
            }
            while o > self.store_base && !self.store.is_empty() {
                self.store.pop_front();
                self.store_base += 1;
            }
            let have = o == self.store_base && self.store.front().is_some_and(|a| a.w > 0.5);
            if have {
                let a = self.store.front().unwrap();
                let inv = 1.0 / a.w;
                for c in 0..2 {
                    for s in 0..4 {
                        y[c] += (self.gains[s] - 1.0) * a.s[s][c] * inv;
                    }
                }
                let e = |s: usize| (a.s[s][0] * inv).powi(2) + (a.s[s][1] * inv).powi(2);
                self.lvl_acc[0] += e(0);
                self.lvl_acc[1] += e(3);
                self.lvl_acc[2] += e(1) + e(2);
            } else if o >= self.delay as u64 * 2 {
                self.status.misses.fetch_add(1, Ordering::Relaxed);
            }
            self.lvl_n += 1;
            if self.lvl_n >= 2048 {
                for i in 0..3 {
                    let db = 10.0 * (self.lvl_acc[i] / self.lvl_n as f32 + 1e-12).log10();
                    self.levels_db[i] += (db - self.levels_db[i]) * 0.3;
                    self.lvl_acc[i] = 0.0;
                }
                self.lvl_n = 0;
            }
            if self.fade > 0 {
                let k = self.fade as f32 / (SR / 50) as f32;
                y = [y[0] * k, y[1] * k];
                self.fade -= 1;
                if self.fade == 0 {
                    for f in self.delay_line.iter_mut() {
                        *f = [0.0; 2];
                    }
                }
            }
            fr[0] = y[0];
            fr[1] = y[1];
        }
    }
}

fn worker(model: std::path::PathBuf, jobs: Receiver<Job>, done: Sender<Done>, st: Arc<NeuralStatus>, skip: bool) {
    let mut d = match Demucs::load(&model, true, 2) {
        Ok(d) => d,
        Err(e) => {
            *st.error.lock().unwrap() = format!("{e:#}");
            crate::log(format!("neural stems: model failed to load: {e:#}"));
            return;
        }
    };
    *st.device.lock().unwrap() = d.device.clone();
    crate::log(format!("neural stems: HT-Demucs on {}", d.device));
    st.ready.store(true, Ordering::Relaxed);
    while let Ok(mut job) = jobs.recv() {
        // real time: if we fell behind, jump to the newest window (older ones would land too late;
        // their slices just play unseparated)
        if skip {
            while let Ok(newer) = jobs.try_recv() {
                job = newer;
            }
        }
        let t = Instant::now();
        match d.separate(&job.audio) {
            Ok(stems) => {
                st.infer_ms.store(t.elapsed().as_millis() as u32, Ordering::Relaxed);
                if done.send(Done { end: job.end, stems }).is_err() {
                    return;
                }
            }
            Err(e) => {
                *st.error.lock().unwrap() = format!("{e:#}");
                // answer anyway (blocking callers count replies); an all-zero result = unseparated
                let _ = done.send(Done { end: job.end, stems: vec![0.0; 4 * 2 * W] });
            }
        }
    }
}

/// `sidechain neural-check <full.onnx> <fwd.onnx>`: our STFT front/back end + the forward-only
/// export must reproduce the full-graph export (both on CPU) on the same window.
/// Do the stems of one window add back up to the window, and at which lag?
pub fn check_sum(fwd: &Path, wav: &str) -> Result<()> {
    let w = crate::wav::read(wav)?;
    let mut mix = vec![0f32; 2 * W];
    for i in 0..W.min(w.samples.len() / 2) {
        mix[i] = w.samples[i * 2];
        mix[W + i] = w.samples[i * 2 + 1];
    }
    let mut d = Demucs::load(fwd, std::env::var("CPU").is_err(), 8)?;
    println!("on {}", d.device);
    for _ in 0..3 {
        let t = Instant::now();
        d.separate(&mix)?;
        println!("  separate() {:.3}s", t.elapsed().as_secs_f32());
    }
    let st = d.separate(&mix)?;
    let sum = |c: usize, i: usize| (0..4).map(|s| st[(s * 2 + c) * W + i]).sum::<f32>();
    let edge = SR;
    for lag in [-2048i64, -1024, -512, -380, -256, 0, 256, 380, 512, 1024, 2048] {
        let (mut e, mut r) = (0f64, 0f64);
        for i in edge..W - edge {
            let j = (i as i64 + lag) as usize;
            for c in 0..2 {
                let x = mix[c * W + i] as f64;
                e += x * x;
                r += (x - sum(c, j) as f64).powi(2);
            }
        }
        println!("lag {lag:>6}: sum-of-stems vs mix SNR {:.1} dB", 10.0 * (e / r).log10());
    }
    for (s, n) in SOURCES.iter().enumerate() {
        let e: f64 = (0..2).map(|c| st[(s * 2 + c) * W..(s * 2 + c + 1) * W].iter().map(|v| (*v as f64).powi(2)).sum::<f64>()).sum();
        println!("  {n:<7} energy share {:.1} dB", 10.0 * (e / mix.iter().map(|v| (*v as f64).powi(2)).sum::<f64>()).log10());
    }
    Ok(())
}

pub fn check(full: &Path, fwd: &Path) -> Result<()> {
    use ort::session::Session;
    // a busy test window: chords + noise bursts + a centred melody, stereo-different
    let mut mix = vec![0f32; 2 * W];
    let mut seed = 1u32;
    for i in 0..W {
        let t = i as f32 / SR as f32;
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let noise = (seed as f32 / u32::MAX as f32 - 0.5) * if (i / 11025) % 2 == 0 { 0.3 } else { 0.0 };
        let mel = (2.0 * std::f32::consts::PI * 440.0 * (1.0 + 0.5 * ((t * 2.0).floor() % 3.0) / 3.0) * t).sin() * 0.2;
        let pad_l = (2.0 * std::f32::consts::PI * 196.0 * t).sin() * 0.15;
        let pad_r = (2.0 * std::f32::consts::PI * 247.0 * t).sin() * 0.15;
        mix[i] = mel + pad_l + noise;
        mix[W + i] = mel + pad_r + noise * 0.8;
    }
    println!("full-graph export (CPU)…");
    let mut s = Session::builder(env()?)?.with_intra_threads(8).map_err(|e| anyhow!("{e}"))?.commit_from_file(full)?;
    let input = ort::value::Tensor::from_array(([1usize, 2, W], mix.clone().into_boxed_slice()))?;
    let out = s.run(ort::inputs!["mix" => input])?;
    let reference: Vec<f32> = out["stems"].try_extract_tensor::<f32>()?.1.to_vec();
    drop(out);
    println!("forward-only export + our STFT (CPU)…");
    let mut d = Demucs::load(fwd, false, 8)?;
    let t = Instant::now();
    let ours = d.separate(&mix)?;
    println!("  separate() {:.2}s on {}", t.elapsed().as_secs_f32(), d.device);
    // ignore the outer 0.5 s (edge padding behaviour differs slightly between exports)
    let edge = SR / 2;
    for (si, name) in SOURCES.iter().enumerate() {
        let (mut maxd, mut e_ref, mut e_err) = (0f32, 0f64, 0f64);
        for c in 0..2 {
            for i in edge..W - edge {
                let r = reference[(si * 2 + c) * W + i];
                let o = ours[(si * 2 + c) * W + i];
                maxd = maxd.max((r - o).abs());
                e_ref += (r as f64).powi(2);
                e_err += ((r - o) as f64).powi(2);
            }
        }
        println!("  {name:<7} max |diff| {maxd:.5}   SNR vs full export {:.1} dB", 10.0 * (e_ref / e_err.max(1e-20)).log10());
    }
    Ok(())
}
