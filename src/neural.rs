//! Neural stems: HT-Demucs v4 (forward-only ONNX export) on the GPU through DirectML.
//!
//! The export (huggingface.co/webnn/stem-separator, MIT) contains only the network; the
//! spectrogram front/back end is reproduced here exactly as demucs' `HTDemucs._spec` /
//! `_magnitude` / `_mask` / `_ispec` do it (torch.stft semantics: periodic Hann, centre +
//! reflect padding, normalized). `neural-check` verifies this against the full-graph export.
//! An fp16 copy of the network (`tools/convert_fp16.py`) is preferred when present: 4x faster on
//! the GPU, 45-52 dB SNR against fp32 (`neural-bench`).
//!
//! Streaming: every `hop` seconds the last 7.8 s window is separated. Each output sample is the
//! weighted blend of every separated window that covers it, weighted by how much past and future
//! context it had in that window, so overlapping windows average (as demucs' own overlap-add
//! does) and a separation that arrives late only means the newest samples get less future
//! context, never a switch to something else. Output = delayed mix + sum((g_i - 1) * stem_i), so
//! with all stem gains at 0 dB the output is the input exactly, just delayed.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Result, anyhow};

use crate::config::GpuPriority;
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

/// `foo.onnx` -> `foo_fp16.onnx` when that exists next to it.
pub fn prefer_fp16(model: &Path) -> PathBuf {
    let fp16 = model.with_file_name(format!("{}_fp16.onnx", model.file_stem().and_then(|s| s.to_str()).unwrap_or("")));
    if fp16.exists() { fp16 } else { model.to_path_buf() }
}

/// Spectrogram front/back end + the ONNX session.
pub struct Demucs {
    session: ort::session::Session,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    win: Vec<f32>,
    pub device: String,
    /// last separate(): spectrogram, network, inverse, ms
    pub stage_ms: [f32; 3],
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

/// HTDemucs._spec for one channel: complex [FR][T] (row-major f-major), torch-normalized.
fn spec(r2c: &dyn RealToComplex<f32>, win: &[f32], x: &[f32], out: &mut [Complex32]) {
    // demucs re-pad: reflect (PAD, PAD + T*HL - W) -> length L1
    let l1 = W + PAD + PAD + T * HL - W;
    let padded: Vec<f32> = (0..l1).map(|i| x[reflect(i as isize - PAD as isize, W)]).collect();
    // torch.stft center=True: reflect pad NFFT/2 on both sides; frames 2..2+T kept
    let half = NFFT / 2;
    let mut buf = vec![0f32; NFFT];
    let mut spec = r2c.make_output_vec();
    let mut scratch = r2c.make_scratch_vec();
    let norm = 1.0 / (NFFT as f32).sqrt();
    for t in 0..T {
        let frame = t + 2;
        for n in 0..NFFT {
            let i = (frame * HL + n) as isize - half as isize;
            buf[n] = padded[reflect(i, l1)] * win[n];
        }
        let _ = r2c.process_with_scratch(&mut buf, &mut spec, &mut scratch);
        for f in 0..FR {
            out[f * T + t] = spec[f] * norm;
        }
    }
}

/// HTDemucs._ispec for one channel: complex [FR][T] -> W samples.
fn ispec(c2r: &dyn ComplexToReal<f32>, win: &[f32], z: &[Complex32], out: &mut [f32]) {
    // pad freq to NFFT/2+1 (zero) and 2 empty frames each side -> T+4 frames
    let frames = T + 4;
    let le = HL * T + 2 * PAD; // istft length requested by demucs
    let half = NFFT / 2;
    let total = le + 2 * half; // centre padding is removed after overlap-add
    let mut acc = vec![0f32; total + NFFT];
    let mut env = vec![0f32; total + NFFT];
    let mut spec = c2r.make_input_vec();
    let mut buf = vec![0f32; NFFT];
    let mut scratch = c2r.make_scratch_vec();
    // torch.istft normalized: frame = irfft(z) (1/N) * sqrt(N); realfft c2r has no 1/N
    let scale = 1.0 / (NFFT as f32).sqrt();
    for fr in 0..frames {
        let has = (2..2 + T).contains(&fr);
        for f in 0..=half {
            spec[f] = if has && f < FR { z[f * T + (fr - 2)] } else { Complex32::new(0.0, 0.0) };
        }
        spec[0].im = 0.0;
        spec[half].im = 0.0;
        let _ = c2r.process_with_scratch(&mut spec, &mut buf, &mut scratch);
        let off = fr * HL;
        for n in 0..NFFT {
            acc[off + n] += buf[n] * scale * win[n];
            env[off + n] += win[n] * win[n];
        }
    }
    // drop torch centre padding, then demucs' PAD
    for i in 0..W {
        let j = half + PAD + i;
        out[i] = if env[j] > 1e-11 { acc[j] / env[j] } else { 0.0 };
    }
}

impl Demucs {
    /// `gpu`: try DirectML first (falls back to CPU if unavailable).
    pub fn load(model: &Path, gpu: bool, cpu_threads: usize) -> Result<Self> {
        use ort::session::Session;
        use ort::session::builder::GraphOptimizationLevel;
        let e = |e: ort::Error<_>| anyhow!("{e}");
        let env = env()?;
        let mk = |dml: bool| -> Result<Session> {
            let mut b = Session::builder(env)?.with_optimization_level(GraphOptimizationLevel::Level3).map_err(e)?;
            if dml {
                b = b
                    // DirectML's whole-graph fusion miscomputes this model (fp32: stems at +90 dB,
                    // fp16: NaN) and isn't faster anyway; unfused it matches the CPU result.
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
        let precision = if model.to_string_lossy().contains("fp16") { "fp16" } else { "fp32" };
        let (session, device) = if gpu {
            match mk(true) {
                Ok(s) => (s, format!("GPU (DirectML, {precision})")),
                Err(err) => (mk(false)?, format!("CPU (DirectML failed: {err})")),
            }
        } else {
            (mk(false)?, format!("CPU ({cpu_threads} threads, {precision})"))
        };
        let mut planner = RealFftPlanner::<f32>::new();
        let win = (0..NFFT).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / NFFT as f32).cos()).collect();
        Ok(Demucs { session, r2c: planner.plan_fft_forward(NFFT), c2r: planner.plan_fft_inverse(NFFT), win, device, stage_ms: [0.0; 3] })
    }

    /// Separate one stereo window (planar L then R, W samples each).
    /// Returns planar stems [s][c][n] (4 x 2 x W).
    pub fn separate(&mut self, mix: &[f32]) -> Result<Vec<f32>> {
        assert_eq!(mix.len(), 2 * W);
        let t0 = Instant::now();
        // spectrogram (CAC: channels L.re L.im R.re R.im), one thread per channel
        let (r2c, c2r, win) = (&*self.r2c, &*self.c2r, &self.win[..]);
        let mut z = vec![vec![Complex32::new(0.0, 0.0); FR * T]; 2];
        std::thread::scope(|sc| {
            for (c, zc) in z.iter_mut().enumerate() {
                let x = &mix[c * W..(c + 1) * W];
                sc.spawn(move || spec(r2c, win, x, zc));
            }
        });
        let mut x = vec![0f32; 4 * FR * T];
        for c in 0..2 {
            for i in 0..FR * T {
                x[(c * 2) * FR * T + i] = z[c][i].re;
                x[(c * 2 + 1) * FR * T + i] = z[c][i].im;
            }
        }
        drop(z);
        let (mean, std) = mean_std(&x);
        for v in x.iter_mut() {
            *v = (*v - mean) / (1e-5 + std);
        }
        let (meant, stdt) = mean_std(mix);
        let xt: Vec<f32> = mix.iter().map(|v| (v - meant) / (1e-5 + stdt)).collect();

        let xin = ort::value::Tensor::from_array(([1usize, 4, FR, T], x.into_boxed_slice()))?;
        let xtin = ort::value::Tensor::from_array(([1usize, 2, W], xt.into_boxed_slice()))?;
        let t1 = Instant::now();
        let out = self.session.run(ort::inputs!["x" => xin, "xt" => xtin])?;
        let t2 = Instant::now();
        let xo = out["x_out"].try_extract_tensor::<f32>()?.1;
        let xto = out["xt_out"].try_extract_tensor::<f32>()?.1;

        // inverse spectrogram + time branch, one thread per source
        let mut stems = vec![0f32; 4 * 2 * W];
        std::thread::scope(|sc| {
            for (s, dst) in stems.chunks_mut(2 * W).enumerate() {
                sc.spawn(move || {
                    let mut zc = vec![Complex32::new(0.0, 0.0); FR * T];
                    for (c, dst) in dst.chunks_mut(W).enumerate() {
                        // x_out channel layout: s*4 + c*2 + (re|im)
                        let re = &xo[((s * 4 + c * 2) * FR * T)..((s * 4 + c * 2 + 1) * FR * T)];
                        let im = &xo[((s * 4 + c * 2 + 1) * FR * T)..((s * 4 + c * 2 + 2) * FR * T)];
                        for i in 0..FR * T {
                            zc[i] = Complex32::new(re[i] * std + mean, im[i] * std + mean);
                        }
                        ispec(c2r, win, &zc, dst);
                        let t = &xto[((s * 2 + c) * W)..((s * 2 + c + 1) * W)];
                        for i in 0..W {
                            dst[i] += t[i] * stdt + meant;
                        }
                    }
                });
            }
        });
        drop(out);
        let ms = |a: Instant, b: Instant| (b - a).as_secs_f32() * 1000.0;
        self.stage_ms = [ms(t0, t1), ms(t1, t2), ms(t2, Instant::now())];
        Ok(stems)
    }
}

// ------------------------------------------------------------------------------- GPU priority

#[link(name = "gdi32")]
unsafe extern "system" {
    fn D3DKMTSetProcessSchedulingPriorityClass(process: *mut std::ffi::c_void, class: i32) -> i32;
}

/// GPU scheduling priority for this process (0 idle .. 2 normal .. 4 high, 5 realtime).
pub fn gpu_priority(class: i32) -> Result<()> {
    let st = unsafe { D3DKMTSetProcessSchedulingPriorityClass(windows::Win32::System::Threading::GetCurrentProcess().0, class) };
    if st == 0 { Ok(()) } else { Err(anyhow!("NTSTATUS {st:#x}")) }
}

/// Workers currently holding the raised priority (the last one out puts it back).
static PRIO_HOLDERS: AtomicU32 = AtomicU32::new(0);

/// The worker's GPU priority policy.
struct Prio {
    mode: GpuPriority,
    raised: bool,
    since: Instant,
    late: u32,
}

impl Prio {
    fn set(&mut self, high: bool) {
        if high == self.raised {
            return;
        }
        if high {
            match gpu_priority(5).or_else(|_| gpu_priority(4)) {
                Ok(()) => {
                    PRIO_HOLDERS.fetch_add(1, Ordering::SeqCst);
                    self.raised = true;
                }
                Err(e) => crate::log(format!("neural stems: couldn't raise GPU priority: {e}")),
            }
        } else {
            self.raised = false;
            if PRIO_HOLDERS.fetch_sub(1, Ordering::SeqCst) == 1 {
                let _ = gpu_priority(2);
            }
        }
        self.since = Instant::now();
        self.late = 0;
    }

    /// Auto: after a separation that took `ms`. Raise after two late ones in a row; once raised,
    /// try normal again every 30 s (the game may have calmed down).
    fn after(&mut self, ms: u32) {
        if self.mode != GpuPriority::Auto {
            return;
        }
        // up to MARGIN_S everything keeps its full look-ahead; allow losing a quarter second of it
        // before cutting ahead of the game (which costs it a frame hitch per separation)
        let budget = (MARGIN_S * 1000.0) as u32 + 250;
        if self.raised {
            if self.since.elapsed().as_secs() >= 30 {
                self.set(false);
                crate::log("neural stems: GPU priority back to normal (probing)");
            }
        } else {
            self.late = if ms > budget { self.late + 1 } else { 0 };
            if self.late >= 2 {
                self.set(true);
                if self.raised {
                    crate::log(format!("neural stems: separations running late ({ms} ms), GPU priority raised"));
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------- streaming

/// Shared status for the UI.
#[derive(Default)]
pub struct NeuralStatus {
    pub ready: AtomicBool,
    pub infer_ms: AtomicU32,
    /// Samples that played unseparated because no separation covered them in time.
    pub misses: AtomicU64,
    pub device: Mutex<String>,
    pub error: Mutex<String>,
    /// % of the audio in the last ~5 s that had its full RIGHT_S of future context (the rest was
    /// separated with less, which is still neural, just a little less accurate).
    pub coverage: AtomicU32,
}

struct Job {
    end: u64,
    audio: Vec<f32>,
}
struct Done {
    end: u64,
    /// planar [drums, vocals, music][L, R][W]; empty = this window failed
    stems: Vec<f32>,
    audio: Vec<f32>,
}

struct Win {
    end: u64,
    stems: Vec<f32>,
    /// output samples it has contributed to so far (a newly arrived window fades in)
    used: u32,
}

/// Seconds of future context a sample gets when separations arrive on time.
pub const RIGHT_S: f32 = 1.0;
/// Samples closer than this to a window's start (little past context) count less.
const LEFT_S: f32 = 0.5;
/// Time a separation may take before the newest samples start losing future context.
const MARGIN_S: f32 = 0.85;
/// A newly arrived window fades into the blend over this many samples.
const ARRIVE: u32 = 4096;

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

pub struct NeuralStems {
    hop: usize,
    right: usize,
    delay: usize,
    blocking: bool,
    /// weight of a sample by its position in a window
    wt: Vec<f32>,
    hist: VecDeque<[f32; 2]>,
    in_count: u64,
    out_count: u64,
    next_job: u64,
    pending: usize,
    jobs: Sender<Job>,
    done: Receiver<Done>,
    spare_stems: Sender<Vec<f32>>,
    spare_audio: Vec<Vec<f32>>,
    delay_line: VecDeque<[f32; 2]>,
    wins: VecDeque<Win>,
    gains: [f32; 3],
    zero_run: usize,
    fade: usize,
    test_drop: bool,
    pub status: Arc<NeuralStatus>,
    pub levels_db: [f32; 3],
    lvl_acc: [f32; 3],
    lvl_n: usize,
    lvl_have: usize,
    cov_have: u32,
    cov_n: u32,
    /// how much of the stem change is applied (ramps instead of snapping at coverage edges)
    sep: f32,
    last_delta: [f32; 2],
}

impl NeuralStems {
    /// Total added latency for a hop, in seconds.
    pub fn latency_s(hop_s: f32) -> f32 {
        RIGHT_S + hop_s + MARGIN_S
    }

    /// `priority`: whether to raise this process' GPU scheduling priority (once the model is warm)
    /// so a game keeping the GPU busy can't stretch a separation from ~0.15 s into seconds.
    pub fn new(model: &Path, hop_s: f32, blocking: bool, priority: GpuPriority, status: Arc<NeuralStatus>) -> Self {
        let hop = (hop_s.clamp(0.5, 6.0) * SR as f32) as usize;
        let right = (RIGHT_S * SR as f32) as usize;
        let left = LEFT_S * SR as f32;
        let delay = (Self::latency_s(hop as f32 / SR as f32) * SR as f32) as usize;
        let wt = (0..W).map(|p| 1e-4 + smooth(p as f32 / left) * smooth((W - p) as f32 / right as f32)).collect();
        let (jtx, jrx) = channel::<Job>();
        let (dtx, drx) = channel::<Done>();
        let (stx, srx) = channel::<Vec<f32>>();
        let st = status.clone();
        let model = model.to_path_buf();
        std::thread::Builder::new()
            .name("neural".into())
            .spawn(move || worker(model, jrx, dtx, srx, st, !blocking, priority))
            .expect("spawn neural");
        status.ready.store(false, Ordering::Relaxed);
        NeuralStems {
            hop,
            right,
            delay,
            blocking,
            wt,
            hist: VecDeque::with_capacity(W + 4096),
            in_count: 0,
            out_count: 0,
            next_job: hop as u64,
            pending: 0,
            jobs: jtx,
            done: drx,
            spare_stems: stx,
            spare_audio: Vec::new(),
            delay_line: VecDeque::with_capacity(delay + 4096),
            wins: VecDeque::new(),
            gains: [1.0; 3],
            zero_run: 0,
            fade: 0,
            test_drop: std::env::var("SIDECHAIN_TEST_DROP").is_ok(),
            status,
            levels_db: [-90.0; 3],
            lvl_acc: [0.0; 3],
            lvl_n: 0,
            lvl_have: 0,
            cov_have: 0,
            cov_n: 0,
            sep: 0.0,
            last_delta: [0.0; 2],
        }
    }

    /// Added latency in samples.
    pub fn delay(&self) -> usize {
        self.delay
    }

    fn take_results(&mut self, wait: bool) {
        loop {
            let r = if wait && self.pending > 0 { self.done.recv().ok() } else { self.done.try_recv().ok() };
            let Some(d) = r else { return };
            self.pending = self.pending.saturating_sub(1);
            if self.spare_audio.len() < 2 {
                self.spare_audio.push(d.audio);
            }
            // test hook: pretend every other separation never arrived
            let dropped = self.test_drop && (d.end / self.hop as u64) % 2 == 1;
            if !d.stems.is_empty() && !dropped {
                self.wins.push_back(Win { end: d.end, stems: d.stems, used: 0 });
            }
            if wait {
                return;
            }
        }
    }

    fn send_job(&mut self) {
        let mut audio = self.spare_audio.pop().unwrap_or_else(|| vec![0f32; 2 * W]);
        let pad = W - self.hist.len();
        audio[..pad].fill(0.0);
        audio[W..W + pad].fill(0.0);
        for (i, f) in self.hist.iter().enumerate() {
            audio[pad + i] = f[0];
            audio[W + pad + i] = f[1];
        }
        if self.jobs.send(Job { end: self.in_count, audio }).is_ok() {
            self.pending += 1;
        }
    }

    /// Process interleaved stereo in place. `target_db` = drums, vocals, music gains.
    pub fn process(&mut self, buf: &mut [f32], target_db: [f32; 3]) {
        let want = target_db.map(|d| 10f32.powf(d.max(-60.0) / 20.0));
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
                self.send_job();
            }
            // a pause shouldn't play out the seconds still queued: fade and drop them
            if self.zero_run == SR / 4 && self.delay_line.iter().any(|f| f[0] != 0.0 || f[1] != 0.0) {
                self.fade = SR / 50;
            }
            self.delay_line.push_back(x);
            if self.delay_line.len() <= self.delay {
                fr[0] = 0.0;
                fr[1] = 0.0;
                continue;
            }
            // output side: input sample `o`, blended from every window covering it
            let mut y = self.delay_line.pop_front().unwrap();
            let o = self.out_count;
            self.out_count += 1;
            for i in 0..3 {
                self.gains[i] += (want[i] - self.gains[i]) * 0.0005;
            }
            if self.blocking {
                while self.pending > 0 && !self.wins.iter().any(|w| w.end >= o + self.right as u64) {
                    self.take_results(true);
                }
            }
            while self.wins.front().is_some_and(|w| w.end <= o) {
                let w = self.wins.pop_front().unwrap();
                let _ = self.spare_stems.send(w.stems);
            }
            let (mut acc, mut wsum, mut ctx) = ([[0f32; 2]; 3], 0f32, 0u64);
            for w in self.wins.iter_mut() {
                if o + (W as u64) < w.end {
                    continue; // starts after o
                }
                let p = (o + W as u64 - w.end) as usize;
                let k = self.wt[p] * (w.used + 1).min(ARRIVE) as f32 / ARRIVE as f32;
                w.used = w.used.saturating_add(1);
                for (s, a) in acc.iter_mut().enumerate() {
                    a[0] += k * w.stems[(s * 2) * W + p];
                    a[1] += k * w.stems[(s * 2 + 1) * W + p];
                }
                wsum += k;
                ctx = ctx.max(w.end - o);
            }
            if wsum > 0.0 {
                let inv = 1.0 / wsum;
                let mut delta = [0f32; 2];
                for (s, a) in acc.iter().enumerate() {
                    let (l, r) = (a[0] * inv, a[1] * inv);
                    delta[0] += (self.gains[s] - 1.0) * l;
                    delta[1] += (self.gains[s] - 1.0) * r;
                    self.lvl_acc[s] += l * l + r * r;
                }
                self.sep = (self.sep + 1.0 / 2048.0).min(1.0);
                y[0] += self.sep * delta[0];
                y[1] += self.sep * delta[1];
                self.last_delta = delta;
                self.lvl_have += 1;
                if ctx >= self.right as u64 {
                    self.cov_have += 1;
                }
            } else {
                // nothing separated this sample (only if separations fall far behind): let the last
                // stem change die away over a few ms instead of snapping to the plain mix
                self.sep *= 0.995;
                y[0] += self.sep * self.last_delta[0];
                y[1] += self.sep * self.last_delta[1];
                if self.status.ready.load(Ordering::Relaxed) {
                    self.status.misses.fetch_add(1, Ordering::Relaxed);
                }
            }
            self.cov_n += 1;
            if self.cov_n >= 5 * SR as u32 {
                self.status.coverage.store(self.cov_have * 100 / self.cov_n, Ordering::Relaxed);
                self.cov_have = 0;
                self.cov_n = 0;
            }
            self.lvl_n += 1;
            if self.lvl_n >= 2048 {
                if self.lvl_have > 0 {
                    for i in 0..3 {
                        let db = 10.0 * (self.lvl_acc[i] / self.lvl_have as f32 + 1e-12).log10();
                        self.levels_db[i] += (db - self.levels_db[i]) * 0.3;
                    }
                }
                self.lvl_acc = [0.0; 3];
                self.lvl_n = 0;
                self.lvl_have = 0;
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

fn worker(model: PathBuf, jobs: Receiver<Job>, done: Sender<Done>, spare: Receiver<Vec<f32>>, st: Arc<NeuralStatus>, skip: bool, mode: GpuPriority) {
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
    let mut d = match Demucs::load(&model, true, 4) {
        Ok(d) => d,
        Err(e) => {
            *st.error.lock().unwrap() = format!("{e:#}");
            crate::log(format!("neural stems: model failed to load: {e:#}"));
            return;
        }
    };
    // first run compiles DirectML's kernels (a long GPU burst): do it before raising priority
    let mut warm = vec![0f32; 2 * W];
    warm[0] = 1e-3;
    let _ = d.separate(&warm);
    let gpu = d.device.starts_with("GPU");
    let mut prio = Prio { mode: if gpu { mode } else { GpuPriority::Normal }, raised: false, since: Instant::now(), late: 0 };
    if prio.mode == GpuPriority::High {
        prio.set(true);
    }
    let base = d.device.clone();
    let show = |p: &Prio| format!("{base}{}", if p.raised { " · high GPU priority" } else { "" });
    *st.device.lock().unwrap() = show(&prio);
    *st.error.lock().unwrap() = String::new();
    crate::log(format!("neural stems: HT-Demucs on {base} ({}, GPU priority {:?})", model.display(), prio.mode));
    st.ready.store(true, Ordering::Relaxed);
    while let Ok(mut job) = jobs.recv() {
        // real time: if we fell behind, jump to the newest window (older ones would land too late)
        if skip {
            while let Ok(newer) = jobs.try_recv() {
                job = newer;
            }
        }
        let mut out = spare.try_recv().unwrap_or_else(|_| vec![0f32; 3 * 2 * W]);
        if job.audio.iter().all(|v| v.abs() < 1e-6) {
            out.fill(0.0); // silence separates into silence; don't wake the GPU for it
        } else {
            let t = Instant::now();
            match d.separate(&job.audio) {
                Ok(s4) => {
                    let ms = t.elapsed().as_millis() as u32;
                    st.infer_ms.store(ms, Ordering::Relaxed);
                    let was = prio.raised;
                    prio.after(ms);
                    if prio.raised != was {
                        *st.device.lock().unwrap() = show(&prio);
                    }
                    // [drums, bass, other, vocals] -> [drums, vocals, music]
                    let src = |s: usize, c: usize| &s4[(s * 2 + c) * W..(s * 2 + c + 1) * W];
                    for c in 0..2 {
                        out[c * W..(c + 1) * W].copy_from_slice(src(0, c));
                        out[(2 + c) * W..(3 + c) * W].copy_from_slice(src(3, c));
                        let (b, o) = (src(1, c), src(2, c));
                        for (i, v) in out[(4 + c) * W..(5 + c) * W].iter_mut().enumerate() {
                            *v = b[i] + o[i];
                        }
                    }
                }
                Err(e) => {
                    *st.error.lock().unwrap() = format!("{e:#}");
                    out = Vec::new();
                }
            }
        }
        if done.send(Done { end: job.end, stems: out, audio: job.audio }).is_err() {
            break;
        }
    }
    prio.set(false);
}

/// `sidechain neural-stream <in.wav> <out.wav> [drums vocals music dB] [--realtime] [--prio normal|auto|high] [--hop s]`:
/// run the streaming separator over a file, offline (waits for every separation) or paced in real
/// time like the live engine, and write the result with the latency removed.
pub fn stream_test(inp: &str, out: &str, db: [f32; 3], realtime: bool, prio: GpuPriority, hop: f32) -> Result<()> {
    let w = crate::wav::read(inp)?;
    if w.rate as usize != SR || w.channels != 2 {
        return Err(anyhow!("need 44.1 kHz stereo"));
    }
    let model = prefer_fp16(Path::new("models/htdemucs_fwd.onnx"));
    let status = Arc::new(NeuralStatus::default());
    let mut n = NeuralStems::new(&model, hop, !realtime, prio, status.clone());
    let t = Instant::now();
    while realtime && !status.ready.load(Ordering::Relaxed) {
        if t.elapsed().as_secs() > 60 || !status.error.lock().unwrap().is_empty() {
            return Err(anyhow!("model didn't load: {}", status.error.lock().unwrap()));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let delay = n.delay();
    // pad the tail with faint noise (silence would trigger the pause fade) so everything plays out
    let mut x = w.samples.clone();
    let mut seed = 7u32;
    for _ in 0..(delay + SR) * 2 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        x.push((seed >> 9) as f32 / (1u32 << 23) as f32 * 1e-4 - 5e-5);
    }
    let mut y = Vec::with_capacity(x.len());
    let start = Instant::now();
    let mut infer = Vec::new();
    for (k, chunk) in x.chunks(2 * 441).enumerate() {
        if realtime {
            let due = std::time::Duration::from_secs_f64((k * 441) as f64 / SR as f64);
            if let Some(wait) = due.checked_sub(start.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        let mut b = chunk.to_vec();
        n.process(&mut b, db);
        y.extend_from_slice(&b);
        if k % 100 == 0 {
            infer.push(status.infer_ms.load(Ordering::Relaxed));
        }
    }
    let samples: Vec<f32> = y[delay * 2..delay * 2 + w.samples.len()].to_vec();
    crate::wav::write(out, SR as u32, 2, &samples)?;
    println!(
        "{} on {}: delay {:.2} s, unseparated {} samples, last coverage {}%, infer ms seen {:?}",
        if realtime { "real time" } else { "offline" },
        status.device.lock().unwrap(),
        delay as f32 / SR as f32,
        status.misses.load(Ordering::Relaxed),
        status.coverage.load(Ordering::Relaxed),
        {
            infer.dedup();
            infer
        }
    );
    Ok(())
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

/// `sidechain neural-bench [wav] [runs]`: time the GPU separator per stage and compare its stems
/// with the CPU result on the same window (DirectML experiments via SIDECHAIN_* env vars).
pub fn bench(fwd: &Path, wav: &str, runs: usize) -> Result<()> {
    let w = crate::wav::read(wav)?;
    let mut mix = vec![0f32; 2 * W];
    for i in 0..W.min(w.samples.len() / 2) {
        mix[i] = w.samples[i * 2];
        mix[W + i] = w.samples[i * 2 + 1];
    }
    let t = Instant::now();
    let reference = Demucs::load(Path::new("models/htdemucs_fwd.onnx"), false, 8)?.separate(&mix)?;
    println!("CPU reference {:.2}s", t.elapsed().as_secs_f32());
    if let Ok(p) = std::env::var("SIDECHAIN_GPUPRIO") {
        println!("GPU priority class {p}: {:?}", gpu_priority(p.parse()?));
    }
    let t = Instant::now();
    let mut d = Demucs::load(fwd, true, 8)?;
    println!("load {:.2}s on {}", t.elapsed().as_secs_f32(), d.device);
    let mut ours = Vec::new();
    for r in 0..=runs {
        let t = Instant::now();
        ours = d.separate(&mix)?;
        let [a, b, c] = d.stage_ms;
        println!(
            "{} {:>7.1} ms  (spec {a:.1} | net {b:.1} | ispec {c:.1})",
            if r == 0 { "warm-up".to_string() } else { format!("run {r:>3}") },
            t.elapsed().as_secs_f32() * 1000.0
        );
    }
    let edge = SR / 2;
    for (si, name) in SOURCES.iter().enumerate() {
        let (mut e_ref, mut e_err) = (0f64, 0f64);
        for c in 0..2 {
            for i in edge..W - edge {
                let r = reference[(si * 2 + c) * W + i];
                let o = ours[(si * 2 + c) * W + i];
                e_ref += (r as f64).powi(2);
                e_err += ((r - o) as f64).powi(2);
            }
        }
        println!("  {name:<7} SNR vs CPU {:.1} dB", 10.0 * (e_ref / e_err.max(1e-20)).log10());
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
