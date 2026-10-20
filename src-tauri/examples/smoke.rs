//! Headless check of the library scanner and playback engine:
//! `cargo run --example smoke -- <music folder>`
use std::time::Duration;

use cadence_lib::audio::{Cmd, DspSettings, Engine, QueueItem, Repeat};
use cadence_lib::library;

fn main() {
    let folder = std::env::args().nth(1).expect("music folder");
    let art = std::env::temp_dir().join("cadence-smoke-art");
    let t0 = std::time::Instant::now();
    let mut tracks = library::scan(&[folder], Vec::new(), |_, _| {});
    let albums = library::build_albums(&mut tracks, &art);
    println!(
        "scan: {} tracks, {} albums in {:?}",
        tracks.len(),
        albums.len(),
        t0.elapsed()
    );
    for a in &albums {
        println!(
            "  album {:?} by {} | {} | {} | accent={:?} tint={:?}",
            a.title, a.artist, a.quality, a.format, a.accent, a.tint
        );
    }
    for t in &tracks {
        println!(
            "  {:>2} {} — {} [{} {:?}bit {:?}Hz {:.1}s]",
            t.track_no.unwrap_or(0),
            t.title,
            t.artist,
            t.codec,
            t.bit_depth,
            t.sample_rate,
            t.duration
        );
    }

    let engine = Engine::start(0.05, DspSettings::default(), Repeat::Off, |_| {});
    let items: Vec<QueueItem> = tracks
        .iter()
        .map(|t| QueueItem {
            id: t.id.clone(),
            path: t.path.clone().into(),
            duration: t.duration,
            album_id: t.album_id.clone(),
            gain: t.gain,
            tone: None,
        })
        .collect();
    engine.send(Cmd::Load { items, index: 0 });
    let show = |label: &str| {
        let s = engine.status();
        println!(
            "{label:<14} idx={:?} playing={} pos={:.2}/{:.1} src={:?} out={:?} err={:?}",
            s.index, s.playing, s.position, s.duration, s.source, s.output, s.error
        );
    };
    std::thread::sleep(Duration::from_millis(2500));
    show("after 2.5s");
    engine.send(Cmd::Seek(120.0));
    std::thread::sleep(Duration::from_millis(1500));
    show("seek 120 +1.5s");
    engine.send(Cmd::Pause);
    std::thread::sleep(Duration::from_millis(1000));
    show("paused 1s");
    engine.send(Cmd::Play);
    engine.send(Cmd::Next);
    std::thread::sleep(Duration::from_millis(1500));
    show("next +1.5s");
    // Gapless hand-over: jump near the end of song 2 and let it roll into song 3.
    let dur = engine.status().duration;
    engine.send(Cmd::Seek(dur - 1.5));
    std::thread::sleep(Duration::from_millis(3000));
    show("rolled over");
}
