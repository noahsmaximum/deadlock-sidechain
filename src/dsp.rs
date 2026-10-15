//! The music chain: HPF -> 4-pole resonant LPF -> low shelf / mid bell / high shelf -> drive ->
//! M/S width -> gain -> peak limiter. Trapezoidal SVFs (Simper) so cutoffs/gains can be swept
//! hard every few samples without zipper noise or blow-ups.

use crate::config::{PARAMS, Param, ParamValues};

const CHUNK: usize = 16; // frames per coefficient update
const SMOOTH_MS: f32 = 25.0; // parameter glide

#[derive(Clone, Copy, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

#[derive(Clone, Copy, Default)]
struct Coef {
    a1: f32,
    a2: f32,
    a3: f32,
    m0: f32,
    m1: f32,
    m2: f32,
}

impl Coef {
    fn new(g: f32, k: f32, m0: f32, m1: f32, m2: f32) -> Self {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Coef { a1, a2, a3: g * a2, m0, m1, m2 }
    }
    fn g(fc: f32, fs: f32) -> f32 {
        (std::f32::consts::PI * fc.min(fs * 0.49) / fs).tan()
    }
    fn lowpass(fc: f32, q: f32, fs: f32) -> Self {
        Self::new(Self::g(fc, fs), 1.0 / q, 0.0, 0.0, 1.0)
    }
    fn highpass(fc: f32, q: f32, fs: f32) -> Self {
        let k = 1.0 / q;
        Self::new(Self::g(fc, fs), k, 1.0, -k, -1.0)
    }
    fn bell(fc: f32, q: f32, db: f32, fs: f32) -> Self {
        let a = 10f32.powf(db / 40.0);
        let k = 1.0 / (q * a);
        Self::new(Self::g(fc, fs), k, 1.0, k * (a * a - 1.0), 0.0)
    }
    fn low_shelf(fc: f32, q: f32, db: f32, fs: f32) -> Self {
        let a = 10f32.powf(db / 40.0);
        let k = 1.0 / q;
        Self::new(Self::g(fc, fs) / a.sqrt(), k, 1.0, k * (a - 1.0), a * a - 1.0)
    }
    fn high_shelf(fc: f32, q: f32, db: f32, fs: f32) -> Self {
        let a = 10f32.powf(db / 40.0);
        let k = 1.0 / q;
        Self::new(Self::g(fc, fs) * a.sqrt(), k, a * a, k * (1.0 - a) * a, 1.0 - a * a)
    }
}

impl Svf {
    #[inline]
    fn tick(&mut self, c: &Coef, v0: f32) -> f32 {
        let v3 = v0 - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        c.m0 * v0 + c.m1 * v1 + c.m2 * v2
    }
}

const STAGES: usize = 6; // hp, lp1, lp2, low shelf, bell, high shelf

pub struct Chain {
    fs: f32,
    cur: ParamValues,
    smooth: f32,
    coefs: [Coef; STAGES],
    state: [[Svf; STAGES]; 2],
    gain: f32,
    width: f32,
    drive: f32,
    lim_env: f32,
    lim_rel: f32,
    pub bypass: bool,
}

impl Chain {
    pub fn new(fs: f32) -> Self {
        let mut cur = [0.0; PARAMS.len()];
        for p in PARAMS {
            cur[p.index()] = p.neutral();
        }
        let mut c = Chain {
            fs,
            cur,
            smooth: 1.0 - (-(CHUNK as f32) / (SMOOTH_MS * 0.001 * fs)).exp(),
            coefs: [Coef::default(); STAGES],
            state: [[Svf::default(); STAGES]; 2],
            gain: 1.0,
            width: 1.0,
            drive: 0.0,
            lim_env: 0.0,
            lim_rel: (-1.0 / (0.15 * fs)).exp(),
            bypass: false,
        };
        c.update_coefs();
        c
    }

    fn update_coefs(&mut self) {
        let p = |x: Param| self.cur[x.index()];
        let fs = self.fs;
        let hp = 20.0 * 2f32.powf(p(Param::HighpassOct));
        let lp = 20_000.0 * 2f32.powf(p(Param::LowpassOct));
        let res = p(Param::Resonance);
        self.coefs = [
            Coef::highpass(hp, 0.707, fs),
            // 4th-order Butterworth pair; resonance lifts the second stage's Q
            Coef::lowpass(lp, 0.5412, fs),
            Coef::lowpass(lp, 1.3066 * (1.0 + 3.0 * res), fs),
            Coef::low_shelf(120.0, 0.707, p(Param::BassDb), fs),
            Coef::bell(1000.0, 0.8, p(Param::MidDb), fs),
            Coef::high_shelf(6000.0, 0.707, p(Param::TrebleDb), fs),
        ];
        self.gain = 10f32.powf(p(Param::GainDb) / 20.0);
        self.width = p(Param::Width);
        self.drive = p(Param::Drive);
    }

    /// Process interleaved stereo in place, gliding toward `target`.
    pub fn process(&mut self, buf: &mut [f32], target: &ParamValues) {
        for block in buf.chunks_mut(CHUNK * 2) {
            for i in 0..self.cur.len() {
                self.cur[i] += (target[i] - self.cur[i]) * self.smooth;
            }
            self.update_coefs();
            if self.bypass {
                continue;
            }
            let k = 1.0 + 4.0 * self.drive;
            for fr in block.chunks_exact_mut(2) {
                let mut lr = [fr[0], fr[1]];
                for (ch, x) in lr.iter_mut().enumerate() {
                    let st = &mut self.state[ch];
                    let mut v = *x;
                    for s in 0..STAGES {
                        v = st[s].tick(&self.coefs[s], v);
                    }
                    // unity small-signal gain saturation, blended in by `drive`
                    v += self.drive * ((k * v).tanh() / k - v);
                    *x = v;
                }
                let m = 0.5 * (lr[0] + lr[1]);
                let sd = 0.5 * (lr[0] - lr[1]) * self.width;
                let (mut l, mut r) = ((m + sd) * self.gain, (m - sd) * self.gain);
                // peak limiter at -1 dBFS: instant attack, 150 ms release, soft-clip backstop
                let peak = l.abs().max(r.abs());
                self.lim_env = if peak > self.lim_env { peak } else { peak + (self.lim_env - peak) * self.lim_rel };
                let ceil = 0.891;
                if self.lim_env > ceil {
                    let g = ceil / self.lim_env;
                    l *= g;
                    r *= g;
                }
                fr[0] = soft_clip(l);
                fr[1] = soft_clip(r);
            }
        }
    }
}

#[inline]
fn soft_clip(x: f32) -> f32 {
    if x.abs() < 0.95 { x } else { x.signum() * (0.95 + 0.05 * ((x.abs() - 0.95) / 0.05).tanh()) }
}
