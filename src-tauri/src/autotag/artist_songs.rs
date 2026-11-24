//! Fix Song Info: every song of the file's artists in the iTunes catalogue. A song the catalogue
//! lists under another name (幾田りら's 「宝石」, tagged "Gemstone") isn't found by its words, and a
//! search for the artist's name lists only their best-known 50 songs; the artist's own list has
//! them all, so one of about the same length can be offered.

use super::{itunes_songs, Candidate};

/// The stores asked, as in the search by credits: names in Latin letters, then in Japanese ones.
const COUNTRIES: [&str; 2] = ["US", "JP"];
/// Credits looked up (the main artist and the first one featured).
const MOST_ARTISTS: usize = 2;
/// Songs read per artist and store (iTunes' own cap for a lookup).
const MOST_SONGS: u32 = 200;

/// The songs of each of `credits` in the iTunes stores, all asked at once. Whether a song really
/// shares an artist with the file is for the caller to check: the artist found for a name may be
/// another one.
pub(super) fn by_artists(client: &reqwest::blocking::Client, credits: &[String]) -> Vec<Candidate> {
    let asks: Vec<(&str, &str)> = credits
        .iter()
        .filter(|name| !name.trim().is_empty())
        .take(MOST_ARTISTS)
        .flat_map(|name| COUNTRIES.iter().map(move |country| (name.as_str(), *country)))
        .collect();
    std::thread::scope(|s| {
        let running: Vec<_> = asks.iter().map(|(name, country)| s.spawn(move || songs_of(client, name, country))).collect();
        running.into_iter().filter_map(|r| r.join().ok()).flatten().collect()
    })
}

/// The songs of the artist the store finds first for `name`; none when there's no such artist or
/// the store can't be reached.
fn songs_of(client: &reqwest::blocking::Client, name: &str, country: &str) -> Vec<Candidate> {
    let url = format!(
        "https://itunes.apple.com/search?entity=musicArtist&limit=1&country={country}&term={}",
        urlencoding::encode(name)
    );
    let found: Option<serde_json::Value> = client.get(&url).send().ok().and_then(|r| r.json().ok());
    let Some(id) = found.as_ref().and_then(first_artist) else { return Vec::new() };
    let url = format!("https://itunes.apple.com/lookup?id={id}&entity=song&limit={MOST_SONGS}&country={country}");
    itunes_songs(client, &url, country).unwrap_or_default()
}

/// The id of the first artist in an iTunes artist search's answer.
fn first_artist(answer: &serde_json::Value) -> Option<u64> {
    answer["results"].as_array()?.iter().find_map(|r| r["artistId"].as_u64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_first_artist_found_is_used() {
        let answer = json!({ "resultCount": 2, "results": [
            { "wrapperType": "artist", "artistName": "Lilas Ikuta", "artistId": 1489581870u64 },
            { "wrapperType": "artist", "artistName": "Lilas", "artistId": 1704820090u64 }
        ]});
        assert_eq!(first_artist(&answer), Some(1489581870));
    }

    #[test]
    fn an_answer_without_an_artist_gives_none() {
        assert_eq!(first_artist(&json!({ "resultCount": 0, "results": [] })), None);
        assert_eq!(first_artist(&json!({ "errorMessage": "Invalid value" })), None);
        assert_eq!(first_artist(&json!({ "results": [{ "artistName": "No id" }] })), None);
    }
}
