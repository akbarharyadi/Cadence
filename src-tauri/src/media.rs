//! Windows media controls (SMTC): media keys, the Win11 media overlay and the lock screen.

use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

use crate::audio::Cmd;

/// What the overlay should show.
#[derive(Clone, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover: Option<String>,
    pub duration: f64,
}

pub enum Update {
    Track(Option<NowPlaying>),
    Playback { playing: bool, position: f64 },
}

/// Starts the media-controls thread. Button presses are turned into engine commands via `send`.
/// `hwnd` is the main window handle (as an integer, so it can cross threads).
pub fn start(hwnd: isize, send: impl Fn(Cmd) + Send + 'static) -> Sender<Update> {
    let (tx, rx) = crossbeam_channel::unbounded::<Update>();
    std::thread::Builder::new()
        .name("cadence-media".into())
        .spawn(move || run(hwnd, rx, send))
        .expect("media thread");
    tx
}

fn run(hwnd: isize, rx: Receiver<Update>, send: impl Fn(Cmd) + Send + 'static) {
    let config = PlatformConfig {
        display_name: "Cadence",
        dbus_name: "cadence",
        hwnd: (hwnd != 0).then_some(hwnd as *mut std::ffi::c_void),
    };
    let mut controls = match MediaControls::new(config) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("media controls unavailable: {e:?}");
            return;
        }
    };
    let attached = controls.attach(move |event| {
        let cmd = match event {
            MediaControlEvent::Play => Cmd::Play,
            MediaControlEvent::Pause => Cmd::Pause,
            MediaControlEvent::Toggle => Cmd::Toggle,
            MediaControlEvent::Next => Cmd::Next,
            MediaControlEvent::Previous => Cmd::Prev,
            MediaControlEvent::Stop => Cmd::Pause,
            MediaControlEvent::SetPosition(MediaPosition(p)) => Cmd::Seek(p.as_secs_f64()),
            MediaControlEvent::SeekBy(dir, by) => Cmd::SeekBy(match dir {
                SeekDirection::Forward => by.as_secs_f64(),
                SeekDirection::Backward => -by.as_secs_f64(),
            }),
            MediaControlEvent::Seek(dir) => Cmd::SeekBy(if matches!(dir, SeekDirection::Forward) {
                10.0
            } else {
                -10.0
            }),
            _ => return,
        };
        send(cmd);
    });
    if let Err(e) = attached {
        log::warn!("can't attach media controls: {e:?}");
        return;
    }
    let mut duration = 0.0;
    for update in rx {
        let result = match update {
            Update::Track(Some(np)) => {
                duration = np.duration;
                controls.set_metadata(MediaMetadata {
                    title: Some(&np.title),
                    artist: Some(&np.artist),
                    album: Some(&np.album),
                    cover_url: np.cover.as_deref(),
                    duration: (np.duration > 0.0).then(|| Duration::from_secs_f64(np.duration)),
                })
            }
            Update::Track(None) => controls.set_playback(MediaPlayback::Stopped),
            Update::Playback { playing, position } => {
                let progress = Some(MediaPosition(Duration::from_secs_f64(
                    position.clamp(0.0, duration.max(position)),
                )));
                controls.set_playback(if playing {
                    MediaPlayback::Playing { progress }
                } else {
                    MediaPlayback::Paused { progress }
                })
            }
        };
        if let Err(e) = result {
            log::debug!("media controls update failed: {e:?}");
        }
    }
}
