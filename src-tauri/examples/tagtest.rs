//! Writes tags to a temporary copy of a file and reads them back:
//! `cargo run --example tagtest -- <audio file>`
use cadence_lib::{library, tagedit};

fn main() {
    let src = std::path::PathBuf::from(std::env::args().nth(1).expect("audio file"));
    let tmp = std::env::temp_dir().join(format!(
        "cadence-tagtest.{}",
        src.extension().unwrap().to_string_lossy()
    ));
    std::fs::copy(&src, &tmp).expect("copy");
    let before = library::read_track(&tmp).expect("read");
    let changes = tagedit::TagChanges {
        title: Some("Edited Title 編集".into()),
        album_artist: Some("Test Album Artist".into()),
        year: Some("1999".into()),
        track_no: Some("7".into()),
        genre: Some("".into()), // remove
        ..Default::default()
    };
    tagedit::write(&tmp, &changes).expect("write");
    let after = library::read_track(&tmp).expect("read back");
    println!("title: {:?} -> {:?}", before.title, after.title);
    println!(
        "album artist: {:?} -> {:?}",
        before.album_artist, after.album_artist
    );
    println!(
        "year: {:?} -> {:?}, track: {:?} -> {:?}, genre: {:?} -> {:?}",
        before.year, after.year, before.track_no, after.track_no, before.genre, after.genre
    );
    println!(
        "artist unchanged: {}, duration unchanged: {}",
        before.artist == after.artist,
        (before.duration - after.duration).abs() < 0.01
    );
    println!(
        "audio still decodes: {}",
        cadence_lib::audio::loudness::measure(&tmp).is_ok()
    );
    std::fs::remove_file(&tmp).ok();
}
