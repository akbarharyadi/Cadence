//! Crossfade check: `cargo run --example xfade -- <folder A> <folder B>`
use std::time::{Duration, Instant};

use cadence_lib::audio::{Cmd, CrossfadeSettings, DspSettings, Engine, QueueItem, Repeat};
use cadence_lib::library;

fn main() {
    let a = std::env::args().nth(1).expect("folder A");
    let b = std::env::args().nth(2).expect("folder B");
    let mut tracks = library::scan(&[a, b], Vec::new(), |_, _| {});
    let _ = library::build_albums(&mut tracks, &std::env::temp_dir().join("cadence-xf-art"));
    let item = |t: &library::Track| QueueItem { id: t.title.clone(), path: t.path.clone().into(), duration: t.duration, album_id: t.album_id.clone(), gain: t.gain, tone: None };
    let night: Vec<_> = tracks.iter().filter(|t| t.album.starts_with("The Night")).collect();
    let other = tracks.iter().find(|t| !t.album.starts_with("The Night")).unwrap();
    let dsp = DspSettings { crossfade: CrossfadeSettings { enabled: true, seconds: 4.0 }, ..Default::default() };
    let engine = Engine::start(0.03, dsp, Repeat::Off, |_| {});

    let watch = |label: &str, engine: &Engine, secs: f64| {
        let t0 = Instant::now();
        let mut last = None;
        while t0.elapsed().as_secs_f64() < secs {
            let s = engine.status();
            if s.index != last {
                println!("  [{label}] t={:.2}s now playing #{:?} at {:.2}s", t0.elapsed().as_secs_f64(), s.index, s.position);
                last = s.index;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    println!("different albums (should crossfade 4 s before the end):");
    engine.send(Cmd::Load { items: vec![item(night[0]), item(other)], index: 0 });
    std::thread::sleep(Duration::from_millis(800));
    engine.send(Cmd::Seek(night[0].duration - 7.0));
    watch("xf", &engine, 6.0);

    println!("same album (should stay gapless, switching right at the end):");
    engine.send(Cmd::Load { items: vec![item(night[1]), item(night[2])], index: 0 });
    std::thread::sleep(Duration::from_millis(800));
    engine.send(Cmd::Seek(night[1].duration - 7.0));
    watch("gapless", &engine, 8.5);
}
