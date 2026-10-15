//! Real-time approximate stems on the CPU: drums / vocals / music, each with its own gain.
//!
//! STFT (2048, hop 512, Hann/Hann). Per bin and frame:
//! - **drums** = percussive part of a causal harmonic/percussive split (Fitzgerald HPSS):
//!   harmonic estimate = median of the bin's magnitude over the last frames, percussive estimate =
//!   median across neighbouring bins in this frame, soft Wiener mask P = p²/(p²+h²). Causal
//!   (past frames only) so the only latency is the frame itself.
//! - **vocals** = the harmonic part that is centre-panned (L/R coherence 2|LR*|/(|L|²+|R|²),
//!   sharpened) and inside the vocal band.
//! - **music** = everything else.
//!
//! The three masks always sum to 1, so with all gains at 0 dB the output is exactly the input,
//! delayed by one frame (~43 ms). One real gain per bin is applied to both channels, so every
//! component keeps its stereo image. It's an approximation: consonants bleed into drums, centred
//! lead instruments into vocals; soft masks + temporal smoothing keep artefacts down.

use std::collections::VecDeque;
use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

pub const N: usize = 2048;
pub const HOP: usize = 512;
const BINS: usize = N / 2 + 1;
const T_HIST: usize = 11; // frames in the harmonic (time) median, ~117 ms
const F_HALF: usize = 8; // ± bins in the percussive (frequency) median
const CENTER_SHARP: i32 = 6; // coherence exponent: higher = only hard-centre counts as vocal
const MASK_SMOOTH: f32 = 0.5; // per-frame one-pole on masks (fights musical noise)
const GAIN_SMOOTH: f32 = 0.3; // per-frame glide of stem gains (~30 ms)

pub struct Stems {
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    win: Vec<f32>,
    ring: [Vec<f32>; 2],
    ring_i: usize,
    pending: usize,
    ola: [Vec<f32>; 2],
    out: VecDeque<[f32; 2]>,
    hist: Vec<f32>,
    hist_i: usize,
    pm: Vec<f32>,
    vm: Vec<f32>,
    vocband: Vec<f32>,
    mag: Vec<f32>,
    tmp: Vec<f32>,
    fbuf: Vec<f32>,
    spec: [Vec<Complex32>; 2],
    scratch_f: Vec<Complex32>,
    scratch_i: Vec<Complex32>,
    gains_db: [f32; 3],
    /// Smoothed energy per stem in dB (drums, vocals, music) — for meters.
    pub levels_db: [f32; 3],
}

fn band_weight(f: f32) -> f32 {
    // 0 below 120 Hz, up to 1 by 250 Hz, flat to 6 kHz, down to 0 by 12 kHz
    let up = ((f - 120.0) / 130.0).clamp(0.0, 1.0);
    let down = (1.0 - (f - 6000.0) / 6000.0).clamp(0.0, 1.0);
    up.min(down)
}

fn median(v: &mut [f32]) -> f32 {
    let m = v.len() / 2;
    *v.select_nth_unstable_by(m, |a, b| a.total_cmp(b)).1
}

impl Stems {
    pub fn new(fs: f32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(N);
        let c2r = planner.plan_fft_inverse(N);
        // periodic Hann: sum of w^2 at 75% overlap is exactly 1.5
        let win: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos()).collect();
        let vocband = (0..BINS).map(|k| band_weight(k as f32 * fs / N as f32)).collect();
        let scratch_f = r2c.make_scratch_vec();
        let scratch_i = c2r.make_scratch_vec();
        let mut out = VecDeque::with_capacity(N * 2);
        out.extend(std::iter::repeat_n([0.0; 2], HOP));
        Stems {
            spec: [r2c.make_output_vec(), r2c.make_output_vec()],
            r2c,
            c2r,
            win,
            ring: [vec![0.0; N], vec![0.0; N]],
            ring_i: 0,
            pending: 0,
            ola: [vec![0.0; N], vec![0.0; N]],
            out,
            hist: vec![0.0; T_HIST * BINS],
            hist_i: 0,
            pm: vec![0.0; BINS],
            vm: vec![0.0; BINS],
            vocband,
            mag: vec![0.0; BINS],
            tmp: vec![0.0; 2 * F_HALF + 1 + T_HIST],
            fbuf: vec![0.0; N],
            scratch_f,
            scratch_i,
            gains_db: [0.0; 3],
            levels_db: [-90.0; 3],
        }
    }


    /// Process interleaved stereo in place. `target_db` = drums, vocals, music gains.
    pub fn process(&mut self, buf: &mut [f32], target_db: [f32; 3]) {
        for fr in buf.chunks_exact_mut(2) {
            self.ring[0][self.ring_i] = fr[0];
            self.ring[1][self.ring_i] = fr[1];
            self.ring_i = (self.ring_i + 1) % N;
            self.pending += 1;
            if self.pending == HOP {
                self.pending = 0;
                self.frame(target_db);
            }
            let o = self.out.pop_front().unwrap_or([0.0; 2]);
            fr[0] = o[0];
            fr[1] = o[1];
        }
    }

    fn frame(&mut self, target_db: [f32; 3]) {
        for c in 0..2 {
            for i in 0..N {
                // oldest sample first
                self.fbuf[i] = self.ring[c][(self.ring_i + i) % N] * self.win[i];
            }
            let _ = self.r2c.process_with_scratch(&mut self.fbuf, &mut self.spec[c], &mut self.scratch_f);
        }
        for k in 0..BINS {
            self.mag[k] = 0.5 * (self.spec[0][k].norm() + self.spec[1][k].norm());
        }
        let base = self.hist_i * BINS;
        self.hist[base..base + BINS].copy_from_slice(&self.mag);
        self.hist_i = (self.hist_i + 1) % T_HIST;

        for i in 0..3 {
            self.gains_db[i] += (target_db[i] - self.gains_db[i]) * GAIN_SMOOTH;
        }
        let g = self.gains_db.map(|d| 10f32.powf(d.max(-60.0) / 20.0));
        let mut energy = [0f32; 3];
        for k in 0..BINS {
            // harmonic: median over time (past frames)
            for t in 0..T_HIST {
                self.tmp[t] = self.hist[t * BINS + k];
            }
            let h = median(&mut self.tmp[..T_HIST]);
            // percussive: median across frequency (this frame)
            let lo = k.saturating_sub(F_HALF);
            let hi = (k + F_HALF + 1).min(BINS);
            let n = hi - lo;
            self.tmp[..n].copy_from_slice(&self.mag[lo..hi]);
            let p = median(&mut self.tmp[..n]);
            let pmask = p * p / (p * p + h * h + 1e-12);

            let (l, r) = (self.spec[0][k], self.spec[1][k]);
            let cross = (l * r.conj()).norm();
            let pow = l.norm_sqr() + r.norm_sqr();
            let center = (2.0 * cross / (pow + 1e-12)).clamp(0.0, 1.0).powi(CENTER_SHARP);
            let vmask = (1.0 - pmask) * self.vocband[k] * center;

            // drums: instant attack (transients live in one frame), smoothed release
            self.pm[k] = if pmask > self.pm[k] { pmask } else { self.pm[k] + (pmask - self.pm[k]) * MASK_SMOOTH };
            self.vm[k] += (vmask - self.vm[k]) * MASK_SMOOTH;
            self.vm[k] = self.vm[k].min(1.0 - self.pm[k]);
            let mm = (1.0 - self.pm[k] - self.vm[k]).max(0.0);
            let gain = g[0] * self.pm[k] + g[1] * self.vm[k] + g[2] * mm;

            energy[0] += self.pm[k] * self.pm[k] * pow;
            energy[1] += self.vm[k] * self.vm[k] * pow;
            energy[2] += mm * mm * pow;
            self.spec[0][k] = l * gain;
            self.spec[1][k] = r * gain;
        }
        for (i, e) in energy.iter().enumerate() {
            let db = 10.0 * (e / (N * N) as f32 + 1e-12).log10();
            self.levels_db[i] += (db - self.levels_db[i]) * 0.3;
        }

        // inverse; realfft round trip scales by N; Hann² overlap sums to 1.5
        let scale = 1.0 / (N as f32 * 1.5);
        for c in 0..2 {
            // DC / Nyquist must be purely real for the inverse
            self.spec[c][0].im = 0.0;
            self.spec[c][BINS - 1].im = 0.0;
            let _ = self.c2r.process_with_scratch(&mut self.spec[c], &mut self.fbuf, &mut self.scratch_i);
            for i in 0..N {
                self.ola[c][i] += self.fbuf[i] * self.win[i] * scale;
            }
        }
        for i in 0..HOP {
            self.out.push_back([self.ola[0][i], self.ola[1][i]]);
        }
        for c in 0..2 {
            self.ola[c].copy_within(HOP.., 0);
            for x in self.ola[c][N - HOP..].iter_mut() {
                *x = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn run(input: &[f32], db: [f32; 3]) -> Vec<f32> {
        let mut s = Stems::new(FS);
        let mut buf = input.to_vec();
        for chunk in buf.chunks_mut(480 * 2) {
            s.process(chunk, db);
        }
        buf
    }

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32) - 0.5
            })
            .collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }

    /// Unity gains reproduce the input exactly, delayed by N samples.
    #[test]
    fn transparent_at_unity() {
        let frames = 48_000;
        let l = noise(frames, 1);
        let r = noise(frames, 7);
        let input: Vec<f32> = l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect();
        let out = run(&input, [0.0; 3]);
        let d = N;
        let mut worst = 0f32;
        for t in N * 2..frames - d {
            worst = worst.max((out[(t + d) * 2] - input[t * 2]).abs());
            worst = worst.max((out[(t + d) * 2 + 1] - input[t * 2 + 1]).abs());
        }
        assert!(worst < 1e-4, "not transparent: max err {worst}");
    }

    /// Clicks (drums) vs a steady centred tone (vocal-band, harmonic) vs a hard-left tone (music).
    #[test]
    fn separates_roughly() {
        let frames = 96_000;
        let mut l = vec![0f32; frames];
        let mut r = vec![0f32; frames];
        let mut clicks = vec![0f32; frames];
        for t in 0..frames {
            let s = (2.0 * std::f32::consts::PI * 440.0 * t as f32 / FS).sin() * 0.2; // centre
            let m = (2.0 * std::f32::consts::PI * 2200.0 * t as f32 / FS).sin() * 0.2; // left only
            l[t] = s + m;
            r[t] = s;
        }
        for t in (2400..frames).step_by(12_000) {
            for i in 0..64 {
                clicks[t + i] = if i % 2 == 0 { 0.9 } else { -0.9 } * (1.0 - i as f32 / 64.0);
            }
        }
        let input: Vec<f32> = (0..frames).flat_map(|t| [l[t] + clicks[t], r[t] + clicks[t]]).collect();
        // energy of the click windows vs the steady parts, before/after killing drums
        let click_rms = |v: &[f32]| {
            let mut acc = Vec::new();
            for t in (2400..frames - N - 64).step_by(12_000).skip(2) {
                acc.extend_from_slice(&v[(t + N) * 2..(t + N + 64) * 2]);
            }
            rms(&acc)
        };
        let dry = run(&input, [0.0; 3]);
        let no_drums = run(&input, [-40.0, 0.0, 0.0]);
        // isolate the click component: subtract the same processing applied to the tones alone
        let tones: Vec<f32> = (0..frames).flat_map(|t| [l[t], r[t]]).collect();
        let tones_dry = run(&tones, [0.0; 3]);
        let tones_wet = run(&tones, [-40.0, 0.0, 0.0]);
        let diff = |a: &[f32], b: &[f32]| -> Vec<f32> { a.iter().zip(b).map(|(x, y)| x - y).collect() };
        let drop = 20.0 * (click_rms(&diff(&no_drums, &tones_wet)) / click_rms(&diff(&dry, &tones_dry))).log10();
        assert!(drop < -15.0, "drums not reduced: {drop:.1} dB");

        // vocals off: the centred tone (right channel = centre only) should fall a lot,
        // the left-only tone (music) should survive
        let no_vox = run(&input, [0.0, -40.0, 0.0]);
        let seg = |v: &[f32], ch: usize| -> Vec<f32> { (42_000..48_000).map(|t| v[t * 2 + ch]).collect() };
        let right_drop = 20.0 * (rms(&seg(&no_vox, 1)) / rms(&seg(&dry, 1))).log10();
        assert!(right_drop < -10.0, "centre not reduced: {right_drop:.1} dB");
        let side = |v: &[f32]| -> Vec<f32> { (42_000..48_000).map(|t| v[t * 2] - v[t * 2 + 1]).collect() };
        let side_drop = 20.0 * (rms(&side(&no_vox)) / rms(&side(&dry))).log10();
        assert!(side_drop > -3.0, "side music lost: {side_drop:.1} dB");
        println!("drums -40 -> clicks {drop:.1} dB; vocals -40 -> centre {right_drop:.1} dB, side {side_drop:.1} dB");
    }
}
