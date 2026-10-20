//! Times opening a file and decoding its first audio.
//! Usage: cargo run --release --example opentime -- <file>...   (or @list.txt with one path per line)
fn main() {
    let mut paths = Vec::new();
    for a in std::env::args().skip(1) {
        match a.strip_prefix('@') {
            Some(list) => paths.extend(std::fs::read_to_string(list).expect("list").lines().map(String::from)),
            None => paths.push(a),
        }
    }
    for p in paths {
        let t = std::time::Instant::now();
        let mut src = cadence_lib::audio::open_source(std::path::Path::new(&p)).expect("open");
        let opened = t.elapsed();
        // Where the music actually starts (first sample above -80 dBFS).
        let mut frames = 0usize;
        let mut start = None;
        while let Ok(Some((buf, rate, ch))) = src.next_chunk() {
            if let Some(i) = buf.iter().position(|s| s.abs() > 1e-4) {
                start = Some((frames + i / ch) as f64 / rate as f64);
                break;
            }
            frames += buf.len() / ch;
        }
        if let Some(s) = start {
            println!("  music starts at {:.3}s", s);
        }
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        println!("open {opened:?}, first audio {:?}, {:.1} MB  {}", t.elapsed(), size as f64 / 1e6, p.rsplit(std::path::MAIN_SEPARATOR).next().unwrap_or(&p));
    }
}
