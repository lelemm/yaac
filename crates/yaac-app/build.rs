//! On Windows targets: write the application icon (drawn in `src/app_icon.rs`) into the
//! executable's resources, so Explorer, the taskbar, and shortcuts show it. Uses the GNU
//! resource compiler (`$WINDRES`, else `x86_64-w64-mingw32-windres`, else `windres`); when none
//! is found the build goes on without an embedded icon (the window still sets its own).

use std::path::{Path, PathBuf};

#[path = "src/app_icon.rs"]
#[allow(dead_code, unreachable_pub, reason = "shared with the binary")]
mod app_icon;

fn main() {
    println!("cargo:rerun-if-changed=src/app_icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WINDRES");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os != "windows" {
        return;
    }
    let Some(out) = std::env::var_os("OUT_DIR").map(PathBuf::from) else { return };
    if env != "gnu" {
        println!("cargo:warning=no icon resource embedded: only the GNU toolchain's windres is wired up");
        return;
    }
    if let Err(e) = embed(&out) {
        println!("cargo:warning=no icon resource embedded: {e}");
    }
}

fn embed(out: &Path) -> Result<(), String> {
    let ico = out.join("yaac.ico");
    std::fs::write(&ico, icon_file()?).map_err(|e| e.to_string())?;
    let rc = out.join("yaac.rc");
    let ico_path = ico.to_string_lossy().replace('\\', "/");
    std::fs::write(&rc, format!("1 ICON \"{ico_path}\"\n")).map_err(|e| e.to_string())?;
    let obj = out.join("yaac-icon.o");
    let candidates: Vec<String> = std::env::var("WINDRES")
        .ok()
        .into_iter()
        .chain(["x86_64-w64-mingw32-windres".to_owned(), "windres".to_owned()])
        .collect();
    let mut last = String::from("no resource compiler found");
    for windres in candidates {
        match std::process::Command::new(&windres).arg(&rc).args(["-O", "coff", "-o"]).arg(&obj).output() {
            Ok(o) if o.status.success() => {
                println!("cargo:rustc-link-arg-bins={}", obj.display());
                return Ok(());
            }
            Ok(o) => last = format!("{windres}: {}", String::from_utf8_lossy(&o.stderr).trim()),
            Err(e) => last = format!("{windres}: {e}"),
        }
    }
    Err(last)
}

/// An `.ico` with 16–64 px images as 32-bit bitmaps and a 256 px PNG.
fn icon_file() -> Result<Vec<u8>, String> {
    let sizes = [16u32, 24, 32, 48, 64, 256];
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| if s == 256 { png(s) } else { Ok(bmp(s)) }).collect::<Result<_, _>>()?;
    let mut f = Vec::new();
    f.extend_from_slice(&0u16.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&u16::try_from(sizes.len()).map_err(|e| e.to_string())?.to_le_bytes());
    let mut offset = 6 + 16 * sizes.len();
    for (&s, image) in sizes.iter().zip(&images) {
        let dim = u8::try_from(s).unwrap_or(0); // 0 means 256
        f.extend_from_slice(&[dim, dim, 0, 0]);
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&32u16.to_le_bytes());
        f.extend_from_slice(&u32::try_from(image.len()).map_err(|e| e.to_string())?.to_le_bytes());
        f.extend_from_slice(&u32::try_from(offset).map_err(|e| e.to_string())?.to_le_bytes());
        offset += image.len();
    }
    for image in images {
        f.extend_from_slice(&image);
    }
    Ok(f)
}

/// A BITMAPINFOHEADER image: BGRA rows bottom-up, then an all-zero AND mask.
fn bmp(size: u32) -> Vec<u8> {
    let rgba = app_icon::rgba(size);
    let mask_row = size.div_ceil(32) * 4;
    let mut b = Vec::new();
    for v in [40u32, size, size * 2] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    for v in [0u32, size * size * 4 + mask_row * size, 0, 0, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    let row = size as usize * 4;
    for y in (0..size as usize).rev() {
        for px in rgba[y * row..(y + 1) * row].as_chunks::<4>().0 {
            b.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    b.resize(b.len() + (mask_row * size) as usize, 0);
    b
}

fn png(size: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&app_icon::rgba(size)).map_err(|e| e.to_string())?;
    }
    Ok(out)
}
