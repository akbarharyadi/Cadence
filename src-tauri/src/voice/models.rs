//! The downloads behind "Follow the singer": ONNX Runtime (the engine that runs the models), the
//! vocal separator (MDX-Net "Voc_FT" from Ultimate Vocal Remover, as packaged by sherpa-onnx) and
//! the letter model (Meta's MMS forced aligner, CC BY-NC 4.0, fine for Cadence as a non-commercial
//! project). Each is checked against its BLAKE3 hash.
//!
//! The letter model comes from Hugging Face at a fixed revision; ONNX Runtime from Microsoft's
//! release on GitHub, or on Windows from its Python package (15 MB instead of the 83 MB archive).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

pub struct Part {
    url: &'static str,
    size: u64,
    blake3: &'static str,
    /// The file inside the downloaded archive, for ONNX Runtime.
    inside: Option<&'static str>,
    /// Saved under this name.
    pub file: &'static str,
}

#[cfg(all(windows, target_arch = "x86_64"))]
pub const RUNTIME: Option<Part> = Some(Part {
    url: "https://files.pythonhosted.org/packages/59/bf/a1d282b4c753ba3e9b31f4c11abd2bc2390513dc5e3792ca3b78e42401c3/onnxruntime-1.31.1-cp313-cp313-win_amd64.whl",
    size: 14_888_304,
    blake3: "830ab217f78436f5bd57813b56c4f6a2959d402939dbbcb5174e82a7899ce6a1",
    inside: Some("onnxruntime/capi/onnxruntime.dll"),
    file: "onnxruntime.dll",
});

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const RUNTIME: Option<Part> = Some(Part {
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-linux-x64-1.31.1.tgz",
    size: 11_413_655,
    blake3: "193e483b6fdbfa841407c65e1eb44c503b676296f749839136e6de3400d4520a",
    inside: Some("onnxruntime-linux-x64-1.31.1/lib/libonnxruntime.so.1.31.1"),
    file: "libonnxruntime.so",
});

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const RUNTIME: Option<Part> = Some(Part {
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-osx-arm64-1.31.1.tgz",
    size: 36_936_435,
    blake3: "cb0b0b48503ec3258394eb8f94f0a67749ce605a0cffa19727b7c17a2241eef2",
    inside: Some("onnxruntime-osx-arm64-1.31.1/lib/libonnxruntime.1.31.1.dylib"),
    file: "libonnxruntime.dylib",
});

/// No ONNX Runtime build for this system (Intel Macs, ARM Windows and Linux, phones).
#[cfg(not(any(
    all(windows, target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
pub const RUNTIME: Option<Part> = None;

pub const SEPARATOR: Part = Part {
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/source-separation-models/UVR-MDX-NET-Voc_FT.onnx",
    size: 66_762_795,
    blake3: "5cde60e795afa4aa78b83d17f8a77999fb8279239580203f3e95d9a1e3890ee9",
    inside: None,
    file: "UVR-MDX-NET-Voc_FT.onnx",
};

pub const LETTERS: Part = Part {
    url: "https://huggingface.co/Vivek2259/mms-fa-mm8/resolve/b7b937dc01d8d39b58e4912c146bedae20733a38/mms_fa_mm8.onnx",
    size: 356_230_951,
    blake3: "98d360889af699aa949a80e9f1af1699102d79a45fd95e10e001c0f281c340c0",
    inside: None,
    file: "mms_fa_mm8.onnx",
};

pub fn parts() -> Vec<&'static Part> {
    match &RUNTIME {
        Some(r) => vec![r, &SEPARATOR, &LETTERS],
        None => Vec::new(),
    }
}

/// Bytes to download for everything.
pub fn download_size() -> u64 {
    parts().iter().map(|p| p.size).sum()
}

pub fn path(dir: &Path, part: &Part) -> PathBuf {
    dir.join(part.file)
}

/// Everything is there (files are checked when downloaded; a file of the right size is trusted).
pub fn installed(dir: &Path) -> bool {
    let parts = parts();
    !parts.is_empty() && parts.iter().all(|p| installed_part(dir, p))
}

/// Downloads what's missing. `progress(done, total)` is called as bytes arrive; returning false
/// cancels.
pub fn install(dir: &Path, mut progress: impl FnMut(u64, u64) -> bool) -> Result<()> {
    let parts = parts();
    if parts.is_empty() {
        bail!("Following the singer isn’t available on this system yet.");
    }
    std::fs::create_dir_all(dir)?;
    let total: u64 = parts.iter().map(|p| p.size).sum();
    let mut done = 0;
    let client = crate::net::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION"), " (open-source music player)"))
        .build()?;
    for part in parts {
        if installed_part(dir, part) {
            done += part.size;
            progress(done, total);
            continue;
        }
        let tmp = dir.join(format!("{}.download", part.file));
        let result = fetch(&client, part, &tmp, |n| progress(done + n, total));
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        match part.inside {
            Some(inside) => {
                let out = path(dir, part);
                let extracted = extract(&tmp, inside, &out);
                let _ = std::fs::remove_file(&tmp);
                extracted.with_context(|| format!("couldn't unpack {}", part.file))?;
            }
            None => std::fs::rename(&tmp, path(dir, part))?,
        }
        done += part.size;
        progress(done, total);
    }
    Ok(())
}

fn installed_part(dir: &Path, p: &Part) -> bool {
    let f = path(dir, p);
    match p.inside {
        Some(_) => f.is_file(),
        None => std::fs::metadata(&f).is_ok_and(|m| m.len() == p.size),
    }
}

fn fetch(client: &reqwest::blocking::Client, part: &Part, to: &Path, mut progress: impl FnMut(u64) -> bool) -> Result<()> {
    let mut response = client
        .get(part.url)
        .send()
        .map_err(|e| anyhow!("Couldn’t download {}: {e}", part.file))?;
    if !response.status().is_success() {
        bail!("Couldn’t download {} (HTTP {})", part.file, response.status());
    }
    let mut file = std::fs::File::create(to)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut got = 0u64;
    loop {
        let n = response.read(&mut buf).map_err(|e| anyhow!("Download of {} broke off: {e}", part.file))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        got += n as u64;
        if !progress(got) {
            bail!("cancelled");
        }
    }
    file.flush()?;
    if got != part.size || hasher.finalize().to_hex().as_str() != part.blake3 {
        bail!("{} didn’t download correctly (got {got} bytes). Try again.", part.file);
    }
    Ok(())
}

#[cfg(windows)]
fn extract(archive: &Path, inside: &str, out: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
    let mut entry = zip.by_name(inside)?;
    let tmp = out.with_extension("part");
    std::io::copy(&mut entry, &mut std::fs::File::create(&tmp)?)?;
    std::fs::rename(&tmp, out)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn extract(archive: &Path, inside: &str, out: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
    let mut tar = tar::Archive::new(gz);
    for entry in tar.entries()? {
        let mut entry = entry?;
        if entry.path()?.to_string_lossy() == inside {
            let tmp = out.with_extension("part");
            std::io::copy(&mut entry, &mut std::fs::File::create(&tmp)?)?;
            std::fs::rename(&tmp, out)?;
            return Ok(());
        }
    }
    bail!("{inside} not found in the archive")
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn extract(_: &Path, _: &str, _: &Path) -> Result<()> {
    bail!("not available on this system")
}

/// Deletes the downloads. ONNX Runtime can't be deleted while Cadence has it loaded: it's
/// marked and deleted when Cadence next starts (`finish_removal`).
pub fn remove(dir: &Path) {
    for p in parts() {
        let f = path(dir, p);
        if std::fs::remove_file(&f).is_err() && f.exists() {
            let _ = std::fs::write(dir.join("remove"), b"");
        }
    }
    let _ = std::fs::remove_dir(dir);
}

pub fn finish_removal(dir: &Path) {
    if dir.join("remove").exists() {
        let _ = std::fs::remove_dir_all(dir);
    }
}
