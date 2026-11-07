//! Plays a real single-file (CUE sheet) album through the real engine, quietly, and checks that songs
//! start and end where the cue says: `cargo run --release --example cuecheck -- "<album folder>"`
use std::time::{Duration, Instant};

use cadence_lib::audio::{Cmd, DspSettings, Engine, QueueItem, Repeat};
use cadence_lib::library;

fn main() {
    let folder = std::env::args().nth(1).expect("album folder");
    let mut songs: Vec<_> = library::scan(&[folder], Vec::new(), |_, _| {}).into_iter().filter(|t| t.cue.is_some()).collect();
    songs.sort_by_key(|t| t.track_no);
    assert!(songs.len() >= 3, "needs a folder with a CUE album (found {} songs)", songs.len());
    for t in songs.iter().take(4) {
        let c = t.cue.unwrap();
        println!("  {:02} {} | {:.2}s from {:.2}s to {:?}", t.track_no.unwrap_or(0), t.title, t.duration, c.start, c.end);
    }
    let items: Vec<QueueItem> = songs
        .iter()
        .map(|t| QueueItem {
            id: t.id.clone(),
            path: t.path.clone().into(),
            duration: t.duration,
            album_id: t.album_id.clone(),
            gain: t.gain,
            tone: None,
            start: t.cue.unwrap().start,
            end: t.cue.unwrap().end,
        })
        .collect();
    let engine = Engine::start(0.03, DspSettings::default(), Repeat::Off, false, |_| {});
    let mut ok = true;
    let mut check = |name: &str, pass: bool, detail: String| {
        println!("{} {name}: {detail}", if pass { "PASS" } else { "FAIL" });
        ok &= pass;
    };

    // 1. The first song starts at its own beginning.
    engine.send(Cmd::Load { items: items.clone(), index: 0 });
    std::thread::sleep(Duration::from_millis(2000));
    let s = engine.status();
    check("first song starts at 0", s.index == Some(0) && s.position > 1.0 && s.position < 3.5, format!("idx {:?} pos {:.2}", s.index, s.position));
    check("duration is the song's, not the file's", (s.duration - songs[0].duration).abs() < 0.01, format!("{:.2} vs {:.2}", s.duration, songs[0].duration));

    // 2. Near the end of song 1: song 2 must take over when song 1 ends, not when the file does.
    let lead = 3.0;
    engine.send(Cmd::Seek(songs[0].duration - lead));
    std::thread::sleep(Duration::from_millis(300));
    let before = engine.status();
    let t0 = Instant::now();
    let mut changed = None;
    while t0.elapsed() < Duration::from_secs(10) {
        let s = engine.status();
        if s.index == Some(1) {
            changed = Some((t0.elapsed().as_secs_f64(), s.position));
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    match changed {
        Some((after, pos)) => {
            let expected = lead - (before.position - (songs[0].duration - lead)).max(0.0);
            check("song 1 hands over to song 2 at its end", (after - expected).abs() < 0.6, format!("after {after:.2}s (expected about {expected:.2}s), song 2 position {pos:.2}s"));
            check("song 2 starts from its own beginning", pos < 1.0, format!("position {pos:.2}"));
        }
        None => check("song 1 hands over to song 2 at its end", false, "no hand-over within 10 s (song 1 ran past its end)".into()),
    }

    // 3. Seeking inside a later song is relative to that song.
    engine.send(Cmd::Jump(2));
    std::thread::sleep(Duration::from_millis(800));
    engine.send(Cmd::Seek(30.0));
    std::thread::sleep(Duration::from_millis(1000));
    let s = engine.status();
    check("seek inside song 3", s.index == Some(2) && s.position > 30.5 && s.position < 33.0, format!("idx {:?} pos {:.2}", s.index, s.position));

    // 4. The last song of the file plays to the file's end and the queue stops there.
    let last = items.len() - 1;
    engine.send(Cmd::Jump(last));
    std::thread::sleep(Duration::from_millis(800));
    let dur = songs[last].duration;
    engine.send(Cmd::Seek(dur - 2.0));
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(6) && engine.status().playing {
        std::thread::sleep(Duration::from_millis(50));
    }
    let s = engine.status();
    check("last song stops at the end of the file", !s.playing && t0.elapsed() < Duration::from_secs(5), format!("playing {} after {:.1}s", s.playing, t0.elapsed().as_secs_f64()));
    engine.send(Cmd::Pause);
    println!("{}", if ok { "ALL PASSED" } else { "SOME FAILED" });
    std::process::exit(if ok { 0 } else { 1 });
}
