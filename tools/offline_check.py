"""Offline end-to-end check of control + DSP (no game, no audio devices).

Generates decorrelated stereo pink noise, runs a scripted match timeline through
`sidechain render`, and measures band levels / stereo width / peaks per window versus the dry
input. Usage:  python tools/offline_check.py <sidechain.exe> <workdir>
"""
import subprocess
import sys
from pathlib import Path

import numpy as np

RATE = 48_000
SECONDS = 22

SCRIPT = """
0.00 {"live":1,"hp":1000,"hpm":1000,"ammo":30,"ammom":30,"dead":0}
# spray: 5 shots over 250 ms
2.00 {"ammo":29}
2.05 {"ammo":28}
2.10 {"ammo":27}
2.15 {"ammo":26}
2.20 {"ammo":25}
# take a 30% chunk
4.00 {"hp":700}
# kill
6.00 {"ev":["kill"]}
# die, respawn 3 s later
9.00 {"dead":1,"hp":0}
12.00 {"dead":0,"hp":1000}
# enemy guardian down
14.00 {"ev":[["txt","team","Enemy Yellow Tier 1","DESTROYED",1]]}
# sprinting at the top of the speed range
20.00 {"spd":650.0}
"""

# (label, start, end) windows to measure
WINDOWS = [
    ("baseline", 0.5, 1.8),
    ("fire", 2.05, 2.35),
    ("damage", 4.05, 4.35),
    ("kill", 6.15, 6.6),
    ("dead", 10.0, 11.8),
    ("objective+", 14.6, 15.6),
    ("speed", 20.8, 21.8),
]
BANDS = [("low<150", 20, 150), ("mid 0.5-2k", 500, 2000), ("high>6k", 6000, 20000)]


def pink(n, rng):
    white = rng.standard_normal(n)
    spec = np.fft.rfft(white)
    f = np.fft.rfftfreq(n, 1 / RATE)
    spec[1:] /= np.sqrt(f[1:])
    spec[0] = 0
    x = np.fft.irfft(spec, n)
    return x / np.sqrt(np.mean(x**2))


def write_wav(path, x):
    x = x.astype("<f4")
    data = x.tobytes()
    hdr = b"RIFF" + (36 + len(data)).to_bytes(4, "little") + b"WAVEfmt " + (16).to_bytes(4, "little")
    hdr += (3).to_bytes(2, "little") + (2).to_bytes(2, "little") + RATE.to_bytes(4, "little")
    hdr += (RATE * 8).to_bytes(4, "little") + (8).to_bytes(2, "little") + (32).to_bytes(2, "little")
    hdr += b"data" + len(data).to_bytes(4, "little")
    Path(path).write_bytes(hdr + data)


def read_wav(path):
    b = Path(path).read_bytes()
    i = b.index(b"data") + 8
    return np.frombuffer(b[i:], dtype="<f4").reshape(-1, 2)


def band_db(x, lo, hi):
    spec = np.abs(np.fft.rfft(x * np.hanning(len(x)))) ** 2
    f = np.fft.rfftfreq(len(x), 1 / RATE)
    return 10 * np.log10(spec[(f >= lo) & (f < hi)].sum() + 1e-20)


def main():
    exe, work = sys.argv[1], Path(sys.argv[2])
    work.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(7)
    n = RATE * SECONDS
    l, r = pink(n, rng), pink(n, rng)
    mid = (l + r) * 0.5
    dry = np.stack([0.7 * mid + 0.3 * l, 0.7 * mid + 0.3 * r], axis=1) * 0.1  # ~-20 dBFS, partly wide
    write_wav(work / "in.wav", dry)
    (work / "script.txt").write_text(SCRIPT)
    subprocess.run([exe, "render", str(work / "in.wav"), str(work / "out.wav"), str(work / "script.txt"),
                    "--trace", str(work / "trace.csv")], check=True)
    wet = read_wav(work / "out.wav")
    assert wet.shape == dry.shape, (wet.shape, dry.shape)
    assert np.isfinite(wet).all(), "non-finite samples"

    print(f"\n{'window':<12}" + "".join(f"{b[0]:>13}" for b in BANDS) + f"{'width S/M':>11}{'peak dBFS':>11}")
    for name, a, b in WINDOWS:
        sl = slice(int(a * RATE), int(b * RATE))
        d, w = dry[sl], wet[sl]
        row = f"{name:<12}"
        for _, lo, hi in BANDS:
            delta = band_db(w.mean(1), lo, hi) - band_db(d.mean(1), lo, hi)
            row += f"{delta:>+12.1f}d"
        # side/mid energy in 100 Hz-16 kHz (pink noise's sub-20 Hz drift would swamp a broadband ratio)
        sm = lambda x: 10 ** ((band_db((x[:, 0] - x[:, 1]) / 2, 100, 16000) - band_db((x[:, 0] + x[:, 1]) / 2, 100, 16000)) / 20)
        row += f"{sm(w) / sm(d):>10.2f}x"
        row += f"{20 * np.log10(np.abs(w).max()):>11.1f}"
        print(row)
    print(f"\noverall peak {20 * np.log10(np.abs(wet).max()):.2f} dBFS (limiter ceiling -1.0)")


if __name__ == "__main__":
    main()
