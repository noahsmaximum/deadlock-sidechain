// Procedural app icon: dark disc with three brass EQ bars. Shared by the window/tray (src/gui) and
// build.rs, which turns it into the .ico embedded in the exe.

/// RGBA pixels, `size` x `size`.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let c = s / 2.0;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = ((fx - c).powi(2) + (fy - c).powi(2)).sqrt();
            let a = (c - d).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let mut col = [22u8, 24, 30];
            for (bx, bh) in [(0.30, 0.34), (0.50, 0.58), (0.70, 0.44)] {
                let half_w = s * 0.075;
                let bottom = s * 0.74;
                let top = bottom - s * bh;
                if (fx - s * bx).abs() < half_w && fy > top && fy < bottom {
                    col = [240, 168, 48];
                }
            }
            let i = ((y * size + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[col[0], col[1], col[2], (a * 255.0) as u8]);
        }
    }
    px
}
