//! Prints ReplayGain 2.0 measurements for the audio files in a folder:
//! `cargo run --release --example loudness -- <folder>`
use std::path::PathBuf;

use cadence_lib::audio::loudness::{Leveling, SongGain};
use cadence_lib::audio::measure_album;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("folder"));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("flac"))
        })
        .collect();
    files.sort();
    let t0 = std::time::Instant::now();
    let refs: Vec<&std::path::Path> = files.iter().map(|p| p.as_path()).collect();
    let (tracks, album) = measure_album(&refs);
    println!("measured {} files in {:?}", files.len(), t0.elapsed());
    let lv = Leveling::default();
    for (f, g) in files.iter().zip(&tracks) {
        let g = g.unwrap();
        let sg = SongGain {
            track: Some(g),
            album,
        };
        println!(
            "{:<60} {:>6.2} LUFS  peak {:>6.2} dBFS  track gain {:>6.2} dB  applied(album ctx) {:>6.2} dB",
            f.file_name()
                .unwrap()
                .to_string_lossy()
                .chars()
                .take(58)
                .collect::<String>(),
            -18.0 - g.gain,
            20.0 * g.peak.log10(),
            g.gain,
            lv.gain_db(&sg, true)
        );
    }
    println!("album gain {:?}", album);
}
