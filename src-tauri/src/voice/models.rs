//! The downloads behind "Follow the singer": ONNX Runtime (the engine that runs the models), the
//! vocal separator (MDX-Net "Voc_FT" from Ultimate Vocal Remover, as packaged by sherpa-onnx) and
//! the letter model (Meta's MMS forced aligner, CC BY-NC 4.0, fine for Cadence as a non-commercial
//! project). Each is checked against its BLAKE3 hash.
//!
//! The letter model comes from Hugging Face at a fixed revision; ONNX Runtime from Microsoft's
//! release on GitHub, or on Windows from its DirectML Python package (25 MB instead of the 83 MB
//! archive), which also runs the models on the graphics card.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

pub struct Part {
    url: &'static str,
    size: u64,
    blake3: &'static str,
    /// For ONNX Runtime, an archive: the files taken out of it (the last one is `file`).
    unpack: &'static [Unpack],
    /// Saved under this name.
    pub file: &'static str,
}

/// A file taken out of a downloaded archive.
pub struct Unpack {
    inside: &'static str,
    file: &'static str,
    size: u64,
}

/// ONNX Runtime with DirectML, so the models run on the graphics card when there's one that can
/// (any DirectX 12 GPU) and on the processor otherwise. DirectML.dll is loaded only when the
/// graphics card is used. 1.24.4 is the last DirectML build Microsoft published.
#[cfg(all(windows, target_arch = "x86_64"))]
pub const RUNTIME: Option<Part> = Some(Part {
    url: "https://files.pythonhosted.org/packages/60/53/2bd2696fac19cf8ca55496a0bcfe431f3aff9579eabbb0e231dc238acf6f/onnxruntime_directml-1.24.4-cp313-cp313-win_amd64.whl",
    size: 25_112_253,
    blake3: "3b505968212ed102c6088854394e0e43e218d0c727d9324198c466eae5fc7eda",
    unpack: &[
        Unpack {
            inside: "onnxruntime/capi/DirectML.dll",
            file: "DirectML.dll",
            size: 18_527_768,
        },
        Unpack {
            inside: "onnxruntime/capi/onnxruntime.dll",
            file: "onnxruntime.dll",
            size: 21_111_840,
        },
    ],
    file: "onnxruntime.dll",
});

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const RUNTIME: Option<Part> = Some(Part {
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-linux-x64-1.31.1.tgz",
    size: 11_413_655,
    blake3: "193e483b6fdbfa841407c65e1eb44c503b676296f749839136e6de3400d4520a",
    unpack: &[Unpack {
        inside: "onnxruntime-linux-x64-1.31.1/lib/libonnxruntime.so.1.31.1",
        file: "libonnxruntime.so",
        size: 29_345_384,
    }],
    file: "libonnxruntime.so",
});

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const RUNTIME: Option<Part> = Some(Part {
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-osx-arm64-1.31.1.tgz",
    size: 36_936_435,
    blake3: "cb0b0b48503ec3258394eb8f94f0a67749ce605a0cffa19727b7c17a2241eef2",
    unpack: &[Unpack {
        inside: "onnxruntime-osx-arm64-1.31.1/lib/libonnxruntime.1.31.1.dylib",
        file: "libonnxruntime.dylib",
        size: 44_797_256,
    }],
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
    unpack: &[],
    file: "UVR-MDX-NET-Voc_FT.onnx",
};

pub const LETTERS: Part = Part {
    url: "https://huggingface.co/Vivek2259/mms-fa-mm8/resolve/b7b937dc01d8d39b58e4912c146bedae20733a38/mms_fa_mm8.onnx",
    size: 356_230_951,
    blake3: "98d360889af699aa949a80e9f1af1699102d79a45fd95e10e001c0f281c340c0",
    unpack: &[],
    file: "mms_fa_mm8.onnx",
};

pub fn parts() -> Vec<&'static Part> {
    match &RUNTIME {
        Some(r) => vec![r, &SEPARATOR, &LETTERS],
        None => Vec::new(),
    }
}

/// Bytes still to download (everything, before the first download).
pub fn download_size(dir: &Path) -> u64 {
    parts().iter().filter(|p| !installed_part(dir, p)).map(|p| p.size).sum()
}

pub fn path(dir: &Path, part: &Part) -> PathBuf {
    dir.join(part.file)
}

/// The models and an ONNX Runtime are there: this one, or the one an earlier Cadence downloaded
/// (`outdated_runtime`). Files are checked when downloaded; a file of the right size is trusted.
pub fn installed(dir: &Path) -> bool {
    let Some(runtime) = &RUNTIME else { return false };
    path(dir, runtime).is_file() && [&SEPARATOR, &LETTERS].iter().all(|p| installed_part(dir, p))
}

/// ONNX Runtime is one an earlier Cadence downloaded (on Windows, the build without DirectML,
/// before 0.2.26). It still runs the models, on the processor; `install` replaces it as long as
/// it isn't loaded.
pub fn outdated_runtime(dir: &Path) -> bool {
    RUNTIME.as_ref().is_some_and(|r| installed(dir) && !installed_part(dir, r))
}

/// Downloads what's missing. `progress(done, total)` is called as bytes arrive (`total` is what's
/// missing); returning false cancels.
pub fn install(dir: &Path, mut progress: impl FnMut(u64, u64) -> bool) -> Result<()> {
    let parts = parts();
    if parts.is_empty() {
        bail!("Following the singer isn’t available on this system yet.");
    }
    std::fs::create_dir_all(dir)?;
    let missing: Vec<&Part> = parts.into_iter().filter(|p| !installed_part(dir, p)).collect();
    let total: u64 = missing.iter().map(|p| p.size).sum();
    let mut done = 0;
    let client = crate::net::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION"), " (open-source music player)"))
        .build()?;
    for part in missing {
        let tmp = dir.join(format!("{}.download", part.file));
        let result = fetch(&client, part, &tmp, |n| progress(done + n, total));
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        if part.unpack.is_empty() {
            std::fs::rename(&tmp, path(dir, part))?;
        } else {
            let extracted = extract(&tmp, part.unpack, dir);
            let _ = std::fs::remove_file(&tmp);
            extracted.with_context(|| format!("couldn't unpack {}", part.file))?;
        }
        done += part.size;
        progress(done, total);
    }
    Ok(())
}

fn installed_part(dir: &Path, p: &Part) -> bool {
    let has = |file: &str, size: u64| std::fs::metadata(dir.join(file)).is_ok_and(|m| m.len() == size);
    if p.unpack.is_empty() {
        has(p.file, p.size)
    } else {
        p.unpack.iter().all(|u| has(u.file, u.size))
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

/// Takes files out of an archive, in order, each replacing the one there before.
#[cfg(windows)]
fn extract(archive: &Path, unpack: &[Unpack], dir: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
    for u in unpack {
        let mut entry = zip.by_name(u.inside)?;
        save_unpacked(&mut entry, u, dir)?;
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn extract(archive: &Path, unpack: &[Unpack], dir: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
    let mut tar = tar::Archive::new(gz);
    let mut found = 0;
    for entry in tar.entries()? {
        let mut entry = entry?;
        let name = entry.path()?.to_string_lossy().into_owned();
        if let Some(u) = unpack.iter().find(|u| u.inside == name) {
            save_unpacked(&mut entry, u, dir)?;
            found += 1;
        }
    }
    if found < unpack.len() {
        bail!("{} not found in the archive", unpack.iter().map(|u| u.inside).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn save_unpacked(from: &mut impl Read, u: &Unpack, dir: &Path) -> Result<()> {
    let out = dir.join(u.file);
    let tmp = out.with_extension("part");
    let n = std::io::copy(from, &mut std::fs::File::create(&tmp)?)?;
    // Replacing a library that's loaded (by another Cadence) fails; the old one stays.
    let saved = if n == u.size { std::fs::rename(&tmp, &out).map_err(anyhow::Error::from) } else { Err(anyhow!("{} has {n} bytes, not {}", u.inside, u.size)) };
    if saved.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    saved
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn extract(_: &Path, _: &[Unpack], _: &Path) -> Result<()> {
    bail!("not available on this system")
}

/// Deletes the downloads. ONNX Runtime can't be deleted while Cadence has it loaded: it's
/// marked and deleted when Cadence next starts (`finish_removal`).
pub fn remove(dir: &Path) {
    for p in parts() {
        let files: Vec<&str> = if p.unpack.is_empty() { vec![p.file] } else { p.unpack.iter().map(|u| u.file).collect() };
        for file in files {
            let f = dir.join(file);
            if std::fs::remove_file(&f).is_err() && f.exists() {
                let _ = std::fs::write(dir.join("remove"), b"");
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}

pub fn finish_removal(dir: &Path) {
    if dir.join("remove").exists() {
        let _ = std::fs::remove_dir_all(dir);
    }
}
