//! Looks up lyrics for a title and length: cargo run --example lyr -- "<title>" <seconds>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dur: f64 = a[2].parse().unwrap();
    for q in cadence_lib::lyrics::title_queries(&a[1]) {
        match cadence_lib::lyrics::search(&q) {
            Ok(h) => println!("{q:?}: {} hits, best {:?}", h.len(), cadence_lib::lyrics::best_hit(&h, &cadence_lib::lyrics::Wanted { title: &a[1], artist: a.get(3).map(String::as_str).unwrap_or(""), album: a.get(4).map(String::as_str).unwrap_or(""), duration: dur }).map(|h| (h.id, &h.title, h.duration, h.synced))),
            Err(e) => println!("{q:?}: error {e}"),
        }
    }
}
