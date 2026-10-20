//! Scrobbling to ListenBrainz (listenbrainz.org).
//!
//! A song counts as listened once you've heard half of it or 4 minutes, whichever comes first
//! (ListenBrainz's rule). Listens that can't be sent (offline) are kept in a file and retried.

use std::path::PathBuf;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use serde::{Deserialize, Serialize};
use serde_json::json;

const API: &str = "https://api.listenbrainz.org/1";
pub const TOKEN_SECRET: &str = "listenbrainz-token";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Listen {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration: f64,
    /// Unix seconds when the song started.
    pub at: u64,
}

pub enum Job {
    NowPlaying(Listen),
    Listened(Listen),
    /// Try sending anything waiting in the offline queue.
    Flush,
}

/// Whether a play counts as a listen.
pub fn counts(listened: f64, duration: f64) -> bool {
    duration >= 30.0 && listened >= (duration / 2.0).min(240.0)
}

pub fn start(queue_file: PathBuf) -> Sender<Job> {
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("cadence-scrobble".into())
        .spawn(move || run(rx, queue_file))
        .expect("scrobble thread");
    tx
}

fn client() -> reqwest::blocking::Client {
    crate::net::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
}

fn metadata(l: &Listen) -> serde_json::Value {
    json!({
        "artist_name": l.artist,
        "track_name": l.title,
        "release_name": l.album,
        "additional_info": {
            "media_player": "Cadence",
            "submission_client": "Cadence",
            "submission_client_version": env!("CARGO_PKG_VERSION"),
            "duration_ms": (l.duration * 1000.0).round() as u64,
        }
    })
}

fn submit(
    http: &reqwest::blocking::Client,
    token: &str,
    kind: &str,
    listens: &[Listen],
) -> Result<(), String> {
    let payload: Vec<_> = listens
        .iter()
        .map(|l| {
            if kind == "playing_now" {
                json!({ "track_metadata": metadata(l) })
            } else {
                json!({ "listened_at": l.at, "track_metadata": metadata(l) })
            }
        })
        .collect();
    let r = http
        .post(format!("{API}/submit-listens"))
        .header("Authorization", format!("Token {token}"))
        .json(&json!({ "listen_type": kind, "payload": payload }))
        .send()
        .map_err(|e| e.to_string())?;
    if r.status().is_success() {
        Ok(())
    } else {
        Err(format!("ListenBrainz answered {}", r.status()))
    }
}

fn run(rx: Receiver<Job>, queue_file: PathBuf) {
    let http = client();
    let mut queue: Vec<Listen> = std::fs::read_to_string(&queue_file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let save = |q: &Vec<Listen>| {
        let _ = std::fs::write(&queue_file, serde_json::to_vec(q).unwrap_or_default());
    };
    loop {
        // Retry the offline queue every few minutes even if nothing new happens.
        let job = match rx.recv_timeout(Duration::from_secs(300)) {
            Ok(j) => j,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Job::Flush,
            Err(_) => return,
        };
        let Some(token) = crate::secrets::get(TOKEN_SECRET) else {
            continue;
        };
        match job {
            Job::NowPlaying(l) => {
                if let Err(e) = submit(&http, &token, "playing_now", &[l]) {
                    log::info!("now-playing not sent: {e}");
                }
            }
            Job::Listened(l) => {
                queue.push(l);
                save(&queue);
            }
            Job::Flush => {}
        }
        if !queue.is_empty() {
            // The API accepts up to 1000 listens per "import" request.
            let kind = if queue.len() == 1 { "single" } else { "import" };
            match submit(&http, &token, kind, &queue[..queue.len().min(1000)]) {
                Ok(()) => {
                    let sent = queue.len().min(1000);
                    queue.drain(..sent);
                    save(&queue);
                }
                Err(e) => log::info!("{} listens waiting to be sent: {e}", queue.len()),
            }
        }
    }
}

/// Checks a token and returns the ListenBrainz user name.
pub fn validate(token: &str) -> Result<String, String> {
    let r = client()
        .get(format!("{API}/validate-token"))
        .header("Authorization", format!("Token {}", token.trim()))
        .send()
        .map_err(|e| format!("Couldn't reach ListenBrainz: {e}"))?;
    let v: serde_json::Value = r.json().map_err(|e| e.to_string())?;
    if v["valid"].as_bool() == Some(true) {
        Ok(v["user_name"].as_str().unwrap_or("").to_string())
    } else {
        Err(
            "ListenBrainz didn’t accept this token. Copy it again from listenbrainz.org/settings."
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::counts;

    #[test]
    fn listen_rule() {
        assert!(!counts(100.0, 25.0)); // too short a song
        assert!(counts(100.0, 200.0)); // half heard
        assert!(!counts(90.0, 200.0));
        assert!(counts(240.0, 900.0)); // 4 minutes of a long song
    }
}
