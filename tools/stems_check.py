"""End-to-end check of the energy -> stems arc (no game, no devices).

Synthesises a "song" whose parts are separable by construction — a centred harmonic "vocal"
(330 Hz stack), a wide pad on non-harmonic notes, kick + noise hi-hats — renders a scripted
calm -> sprint -> fight timeline through `sidechain render`, and reports each part's level in
each phase relative to the pad (so global level/EQ moves cancel out).

Usage:  python tools/stems_check.py <sidechain.exe> <workdir>
"""
import subprocess
import sys
from pathlib import Path

import numpy as np

RATE = 48_000
SECONDS = 16

SCRIPT = """
0.00 {"live":1,"hp":1000,"hpm":1000,"ammo":30,"ammom":30,"dead":0,"spd":0}
# 4 s calm, then sprinting (energy ~0.5), then a fight + kill (energy ~1)
4.00 {"spd":700}
9.00 {"spd":700,"combat":1}
9.00 pulse kill
"""

PHASES = [("calm", 2.0, 3.8), ("sprint", 7.0, 8.8), ("fight+kill", 10.0, 11.5)]


def synth(rng):
    t = np.arange(RATE * SECONDS) / RATE
    # centred "vocal": harmonic stack with vibrato, both channels identical
    f0 = 330 * (1 + 0.004 * np.sin(2 * np.pi * 5.5 * t))
    ph = 2 * np.pi * np.cumsum(f0) / RATE
    vocal = sum(np.sin(k * ph) / k for k in range(1, 9)) * 0.08
    # wide pad: different notes per side
    pad_l = sum(np.sin(2 * np.pi * f * t) for f in (415.3, 740.0)) * 0.05
    pad_r = sum(np.sin(2 * np.pi * f * t) for f in (554.4, 932.3)) * 0.05
    # drums: kick every 0.5 s, hats every 0.25 s (noise, 8-12 kHz)
    drums = np.zeros_like(t)
    for start in np.arange(0, SECONDS, 0.5):
        i = int(start * RATE)
        n = int(0.25 * RATE)
        tt = np.arange(n) / RATE
        drums[i:i + n] += np.sin(2 * np.pi * 55 * tt * (1 + 2 * np.exp(-tt * 40))) * np.exp(-tt * 12) * 0.5
    spec = np.fft.rfft(rng.standard_normal(len(t)))
    f = np.fft.rfftfreq(len(t), 1 / RATE)
    spec[(f < 8000) | (f > 12000)] = 0
    hiss = np.fft.irfft(spec, len(t))
    hiss /= np.abs(hiss).max()
    env = np.zeros_like(t)
    for start in np.arange(0.125, SECONDS, 0.25):
        i = int(start * RATE)
        n = int(0.04 * RATE)
        env[i:i + n] = np.exp(-np.arange(n) / (0.008 * RATE))
    drums += hiss * env * 0.3
    left = vocal + pad_l + drums
    right = vocal + pad_r + drums
    return np.stack([left, right], axis=1).astype("<f4")


def write_wav(path, x):
    data = x.astype("<f4").tobytes()
    hdr = b"RIFF" + (36 + len(data)).to_bytes(4, "little") + b"WAVEfmt " + (16).to_bytes(4, "little")
    hdr += (3).to_bytes(2, "little") + (2).to_bytes(2, "little") + RATE.to_bytes(4, "little")
    hdr += (RATE * 8).to_bytes(4, "little") + (8).to_bytes(2, "little") + (32).to_bytes(2, "little")
    hdr += b"data" + len(data).to_bytes(4, "little")
    Path(path).write_bytes(hdr + data)


def read_wav(path):
    b = Path(path).read_bytes()
    i = b.index(b"data") + 8
    return np.frombuffer(b[i:], dtype="<f4").reshape(-1, 2)


def power_at(x, freqs, width=6.0):
    spec = np.abs(np.fft.rfft(x * np.hanning(len(x)))) ** 2
    f = np.fft.rfftfreq(len(x), 1 / RATE)
    return sum(spec[(f > fr - width) & (f < fr + width)].sum() for fr in freqs) + 1e-20


def band(x, lo, hi):
    spec = np.abs(np.fft.rfft(x * np.hanning(len(x)))) ** 2
    f = np.fft.rfftfreq(len(x), 1 / RATE)
    return spec[(f >= lo) & (f < hi)].sum() + 1e-20


def parts(x):
    mid = (x[:, 0] + x[:, 1]) / 2
    side = (x[:, 0] - x[:, 1]) / 2
    vocal = power_at(mid, [330 * k for k in range(1, 6)])
    pad = power_at(side, [415.3, 554.4, 740.0, 932.3])
    hats = band(mid, 8000, 12000)
    return vocal, pad, hats


def main():
    exe, work = sys.argv[1], Path(sys.argv[2])
    work.mkdir(parents=True, exist_ok=True)
    dry = synth(np.random.default_rng(3))
    write_wav(work / "song.wav", dry)
    (work / "script.txt").write_text(SCRIPT)
    # same render with stems off = reference: every other move (level, width, EQ) is identical
    default = (Path(__file__).resolve().parent.parent / "sidechain.default.toml").read_text(encoding="utf-8")
    (work / "on.toml").write_text(default, encoding="utf-8")
    (work / "off.toml").write_text(default.replace("[stems]\nenabled = true", "[stems]\nenabled = false"), encoding="utf-8")
    for tag in ("on", "off"):
        subprocess.run([exe, "render", str(work / "song.wav"), str(work / f"{tag}.wav"), str(work / "script.txt"),
                        "--config", str(work / f"{tag}.toml")], check=True)
    on, off = read_wav(work / "on.wav"), read_wav(work / "off.wav")
    lat = 2048  # stems frame latency
    print(f"\n{'phase':<12}{'vocals':>10}{'drums(hats)':>13}{'pad':>8}   (dB, stems on vs off)")
    for name, a, b in PHASES:
        x_on = on[int(a * RATE) + lat:int(b * RATE) + lat]
        x_off = off[int(a * RATE):int(b * RATE)]
        v1, p1, h1 = parts(x_on)
        v0, p0, h0 = parts(x_off)
        db = lambda x, y: 10 * np.log10(x / y)
        print(f"{name:<12}{db(v1, v0):>+10.1f}{db(h1, h0):>+13.1f}{db(p1, p0):>+8.1f}")


if __name__ == "__main__":
    main()
