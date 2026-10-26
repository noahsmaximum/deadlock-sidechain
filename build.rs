//! Makes `sidechain.exe` self-contained when the pieces are on disk:
//! - models/htdemucs_fwd_fp16.onnx  -> embedded neural model (cfg `bundled_model`)
//! - dist/sidechain-hook.vpk        -> embedded hook addon, installable from Setup (`bundled_hook`)
//! - ort's DirectML.dll             -> embedded and delay-loaded, unpacked on first run (`bundled_dml`)
//! - the procedural icon            -> .ico resource (Explorer / taskbar icon)
//! Set SIDECHAIN_NO_BUNDLE=1 for a lean dev build.

use std::path::{Path, PathBuf};

include!("src/icon.rs");

fn bundle(cfg: &str, env: &str, path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
    if path.exists() && std::env::var_os("SIDECHAIN_NO_BUNDLE").is_none() {
        println!("cargo:rustc-cfg={cfg}");
        println!("cargo:rustc-env={env}={}", path.display());
    }
}

/// Newest DirectML.dll that ort downloaded.
fn find_directml() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("ort.pyke.io").join("dfbin").join("x86_64-pc-windows-msvc");
    std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("DirectML.dll"))
        .filter(|p| p.exists())
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
}

/// .ico with 16..256 px 32-bit BMP images.
fn write_ico(path: &Path) {
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let mut images = Vec::new();
    for &s in &sizes {
        let rgba = icon_rgba(s);
        let mut b = Vec::new();
        // BITMAPINFOHEADER (height doubled: XOR + AND masks)
        for v in [40u32, s, s * 2] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        for _ in 0..6 {
            b.extend_from_slice(&0u32.to_le_bytes());
        }
        for y in (0..s).rev() {
            for x in 0..s {
                let i = ((y * s + x) * 4) as usize;
                b.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        let mask_row = s.div_ceil(32) * 4;
        b.extend(std::iter::repeat_n(0u8, (mask_row * s) as usize));
        images.push(b);
    }
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, img) in sizes.iter().zip(&images) {
        let d = if *s >= 256 { 0 } else { *s as u8 };
        ico.extend_from_slice(&[d, d, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(img.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        ico.extend_from_slice(&img);
    }
    std::fs::write(path, ico).expect("write icon");
}

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-env-changed=SIDECHAIN_NO_BUNDLE");
    for c in ["bundled_model", "bundled_hook", "bundled_dml"] {
        println!("cargo:rustc-check-cfg=cfg({c})");
    }
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    bundle("bundled_model", "SIDECHAIN_MODEL_FILE", &root.join("models").join("htdemucs_fwd_fp16.onnx"));
    bundle("bundled_hook", "SIDECHAIN_HOOK_FILE", &root.join("dist").join("sidechain-hook.vpk"));
    if let Some(dml) = find_directml() {
        bundle("bundled_dml", "SIDECHAIN_DML_FILE", &dml);
        if std::env::var_os("SIDECHAIN_NO_BUNDLE").is_none() {
            // load DirectML only when the neural stems first need it, after we've unpacked it
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:DirectML.dll");
            println!("cargo:rustc-link-arg-bins=delayimp.lib");
        }
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = out.join("sidechain.ico");
    write_ico(&ico);
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().unwrap())
        .set("ProductName", "Sidechain")
        .set("FileDescription", "Sidechain: Deadlock drives your music")
        .set("OriginalFilename", "Sidechain.exe");
    if let Err(e) = res.compile() {
        println!("cargo:warning=no exe icon: {e}");
    }
}
