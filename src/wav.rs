//! Minimal WAV I/O for the offline `render` command (PCM16/24/32, float32 in; float32 out).

use anyhow::{Result, bail};

pub struct Wav {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>, // interleaved
}

pub fn read(path: &str) -> Result<Wav> {
    let b = std::fs::read(path)?;
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        bail!("{path}: not a RIFF/WAVE file");
    }
    let (mut fmt_tag, mut channels, mut rate, mut bits) = (0u16, 0u16, 0u32, 0u16);
    let mut i = 12;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32::from_le_bytes(b[i + 4..i + 8].try_into()?) as usize;
        let body = &b[i + 8..(i + 8 + len).min(b.len())];
        if id == b"fmt " {
            fmt_tag = u16::from_le_bytes(body[0..2].try_into()?);
            channels = u16::from_le_bytes(body[2..4].try_into()?);
            rate = u32::from_le_bytes(body[4..8].try_into()?);
            bits = u16::from_le_bytes(body[14..16].try_into()?);
            if fmt_tag == 0xFFFE {
                fmt_tag = u16::from_le_bytes(body[24..26].try_into()?); // extensible subformat
            }
        } else if id == b"data" {
            let samples = match (fmt_tag, bits) {
                (1, 16) => body.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect(),
                (1, 24) => body.chunks_exact(3).map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0).collect(),
                (1, 32) => body.chunks_exact(4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0).collect(),
                (3, 32) => body.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
                _ => bail!("{path}: unsupported format tag {fmt_tag} / {bits} bits"),
            };
            return Ok(Wav { rate, channels, samples });
        }
        i += 8 + len + (len & 1);
    }
    bail!("{path}: no data chunk")
}

pub fn write(path: &str, rate: u32, channels: u16, samples: &[f32]) -> Result<()> {
    let data_len = (samples.len() * 4) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * channels as u32 * 4).to_le_bytes());
    b.extend_from_slice(&(channels * 4).to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, b)?;
    Ok(())
}
