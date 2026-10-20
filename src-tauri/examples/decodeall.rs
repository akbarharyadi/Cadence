//! Decodes whole files and reports how far each got: cargo run --release --example decodeall -- @list.txt
fn main() {
    let mut paths = Vec::new();
    for a in std::env::args().skip(1) {
        match a.strip_prefix('@') {
            Some(list) => paths.extend(std::fs::read_to_string(list).expect("list").lines().map(String::from)),
            None => paths.push(a),
        }
    }
    for p in paths {
        let name = p.rsplit(std::path::MAIN_SEPARATOR).next().unwrap_or(&p).to_string();
        let mut src = match cadence_lib::audio::open_source(std::path::Path::new(&p)) {
            Ok(s) => s,
            Err(e) => {
                println!("OPEN FAILED {name}: {e:#}");
                continue;
            }
        };
        let (mut frames, mut rate) = (0usize, 0u32);
        let end = loop {
            match src.next_chunk() {
                Ok(Some((buf, r, ch))) => {
                    frames += buf.len() / ch;
                    rate = r;
                }
                Ok(None) => break "end".to_string(),
                Err(e) => break format!("ERROR {e:#}"),
            }
        };
        println!("{:7.1}s decoded, {end}  {name}", frames as f64 / rate.max(1) as f64);
    }
}
