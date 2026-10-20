//! Analyses a few files and prints their Smart EQ curve and how alike they sound.
//! Usage: cargo run --release --example sonic -- <file>...
fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let round = |x: f32| (x * 10.0).round() / 10.0;
    let mut feats = Vec::new();
    for p in &paths {
        let started = std::time::Instant::now();
        let (f, bands) = cadence_lib::sonic::analyze(std::path::Path::new(p)).expect("analyse");
        let smart = cadence_lib::audio::tone::smart_correction(&bands);
        println!("{:>6.2?}  {}", started.elapsed(), p.rsplit(['/', '\\']).next().unwrap_or(p));
        println!("  bands {:?}", bands.iter().map(|x| round(*x)).collect::<Vec<_>>());
        println!("  smart {:?}", smart.map(round));
        feats.push(f);
    }
    for (i, a) in feats.iter().enumerate() {
        let row: Vec<String> = feats.iter().map(|b| format!("{:.2}", cadence_lib::sonic::distance(a, b))).collect();
        println!("{i}: {}", row.join(" "));
    }
}
