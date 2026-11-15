//! New releases by artists in the library, for Discover ("New from artists you own").
//!
//! ListenBrainz's fresh releases (`GET /explore/fresh-releases/`: the last and next weeks' releases
//! of all MusicBrainz, no account or token needed) are fetched at most once a day, when Discover is
//! opened (never on a timer), and only if "Look for new releases online" is on. Nothing about the
//! listener or the library is sent: it's one question for the whole site. The releases are matched
//! to the library's artists here, by MusicBrainz id first (the id the artist lookup kept), then by
//! name through `identity` (so 米津玄師 and Kenshi Yonezu are one artist), and the ones the library
//! already has (the same album title by the same artist) are left out. What's kept in
//! `new-releases.json` is only the releases of the library's artists, a few hundred bytes each.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::identity::{Identity, Who};
use crate::lbapi::{self, Fail, Fresh, Net, Req};
use crate::musicbrainz::{Clock, Pacer};

/// How far back and ahead it looks (the API allows up to 90).
pub const DAYS: u32 = 30;
const A_DAY: u64 = 86_400;
/// After a failed fetch, not asked again for this long.
const AFTER_FAILURE: u64 = 3_600;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Cache {
    /// When it was last fetched (unix seconds).
    fetched: u64,
    /// When it was last tried, fetched or not.
    attempted: u64,
    days: u32,
    /// The releases of the library's artists, when they were fetched.
    items: Vec<Fresh>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewRelease {
    pub key: String,
    pub title: String,
    pub artist: String,
    pub date: String,
    /// "Album", "Single", "EP"…
    pub kind: String,
    /// Not out yet.
    pub upcoming: bool,
    /// A cover on the Cover Art Archive; the window loads it only when the row is on screen.
    pub cover: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub items: Vec<NewRelease>,
    /// When the releases were fetched (0: never).
    pub fetched: u64,
    /// "Look for new releases online" is on.
    pub enabled: bool,
    /// Why it couldn't ask (empty: fine).
    pub error: String,
}

/// What the library already is.
pub struct Library<'a> {
    pub who: &'a Identity,
    /// (album title, album artist) of every album.
    pub albums: &'a [(String, String)],
}

fn date_of(now: u64) -> String {
    let (y, m, d) = crate::stats::civil((now / A_DAY) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Whether a release is by an artist of the library.
pub fn by_library_artist(who: &Identity, mbids: &HashMap<&str, u32>, r: &Fresh) -> bool {
    if crate::api::no_one(&r.artist) {
        return false;
    }
    // The id first: the artist's own, whatever the name is written like.
    if r.artist_mbids.iter().any(|m| mbids.contains_key(m.as_str())) {
        return true;
    }
    who.credit(&r.artist).into_iter().any(|w| match w {
        Who::Known(i) => {
            // The same name, but another artist (their ids are both known and differ): not theirs.
            let other = who.artist(i).mb.as_ref().is_some_and(|m| !r.artist_mbids.is_empty() && !r.artist_mbids.contains(&m.mbid));
            !other
        }
        Who::Loose(_) => false,
    })
}

/// Whether the library has the album: the same title (without "(Deluxe)", "[Remastered]" and the
/// like) by the same artist.
fn owned(lib: &Library, r: &Fresh) -> bool {
    let want = crate::catalog::core_title(&r.title);
    !want.is_empty() && lib.albums.iter().any(|(title, artist)| crate::catalog::core_title(title) == want && lib.who.same_artist(artist, &r.artist))
}

/// The releases of the library's artists that the library doesn't have, newest first, one per
/// album (editions of one are one).
pub fn select(fresh: &[Fresh], lib: &Library) -> Vec<Fresh> {
    let mbids: HashMap<&str, u32> =
        lib.who.artists().iter().enumerate().filter_map(|(i, a)| a.mb.as_ref().map(|m| (m.mbid.as_str(), i as u32))).collect();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut out: Vec<Fresh> = fresh
        .iter()
        .filter(|r| by_library_artist(lib.who, &mbids, r))
        .filter(|r| seen.insert((lib.who.primary_key(&r.artist), crate::catalog::core_title(&r.title))))
        .cloned()
        .collect();
    out.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.artist.cmp(&b.artist)).then_with(|| a.title.cmp(&b.title)));
    out
}

fn view(cache: &Cache, lib: &Library, enabled: bool, now: u64, error: String) -> View {
    let today = date_of(now);
    let items = cache
        .items
        .iter()
        // (The library may have the album now: it was fetched yesterday, the album arrived today.)
        .filter(|r| !owned(lib, r))
        .map(|r| NewRelease {
            key: if r.release_mbid.is_empty() { format!("{}|{}", r.artist, r.title) } else { r.release_mbid.clone() },
            title: r.title.clone(),
            artist: r.artist.clone(),
            date: r.date.clone(),
            kind: r.kind.clone(),
            upcoming: !r.date.is_empty() && r.date > today,
            cover: r.cover_url(),
        })
        .collect();
    View { items, fetched: cache.fetched, enabled, error }
}

/// Whether it's time to ask again: a day after the last fetch (and an hour after a failed try).
fn due(c: &Cache, now: u64) -> bool {
    let old = c.fetched == 0 || now >= c.fetched.saturating_add(A_DAY) || c.days != DAYS;
    // (A failed try is the only one that leaves the last try later than the last fetch.)
    let rested = c.attempted <= c.fetched || now >= c.attempted.saturating_add(AFTER_FAILURE);
    old && rested
}

/// The fresh releases from the network (one request).
pub fn fetch(net: &mut dyn Net, clock: &mut dyn Clock) -> Result<Vec<Fresh>, Fail> {
    let body = lbapi::request(net, clock, &mut Pacer::default(), &Req::get(lbapi::fresh_path(DAYS), false), &|| false)?;
    lbapi::parse_fresh(&body).ok_or(Fail::Refused(200))
}

/// What Discover shows: the kept releases, fetched anew first when it's due and allowed
/// (`enabled`, and `ask`: the window is asking for it, not only for what's kept).
/// Off: nothing at all, not even what was kept (each kept release's cover would still be loaded
/// from the Cover Art Archive by the window).
pub fn load(file: &Path, now: u64, enabled: bool, ask: bool, net: &mut dyn Net, clock: &mut dyn Clock, lib: &Library) -> View {
    if !enabled {
        return View { enabled: false, ..Default::default() };
    }
    let mut cache: Cache = crate::state::read_json(file);
    let mut error = String::new();
    if ask && due(&cache, now) {
        cache.attempted = now;
        match fetch(net, clock) {
            Ok(all) => {
                cache = Cache { fetched: now, attempted: now, days: DAYS, items: select(&all, lib) };
            }
            Err(Fail::Offline) => error = "Couldn’t reach the release list. Check the connection.".into(),
            Err(Fail::Busy) => error = "The release list is busy right now. It tries again later.".into(),
            Err(_) => error = "The release list answered with an error. It tries again later.".into(),
        }
        // A cache: it can be asked for again, so no .bak.
        crate::state::write_json_with(file, &cache, false);
    }
    view(&cache, lib, enabled, now, error)
}

pub const FILE: &str = "new-releases.json";

fn library_of(ctx: &crate::ctx::Ctx) -> Vec<(String, String)> {
    ctx.state.library.lock().snapshot.albums.iter().map(|a| (a.title.clone(), a.artist.clone())).collect()
}

/// For Discover: asks (when due) and returns what there is.
pub fn for_discover(ctx: &crate::ctx::Ctx, ask: bool) -> View {
    let enabled = ctx.state.settings.lock().new_releases_online;
    if !enabled {
        return View::default();
    }
    let albums = library_of(ctx);
    let who = crate::identity::current();
    let lib = Library { who: &who, albums: &albums };
    let file = ctx.state.data_dir.join(FILE);
    let now = crate::history::now();
    // No client is made (nor network asked) when only what's kept is wanted.
    let made = if ask { lbapi::Http::new(None) } else { None };
    let Some(mut http) = made else {
        return load(&file, now, true, false, &mut NoNet, &mut lbapi::Wall(std::time::Instant::now(), &NEVER), &lib);
    };
    let mut clock = lbapi::Wall(std::time::Instant::now(), &NEVER);
    load(&file, now, enabled, ask, &mut http, &mut clock, &lib)
}

static NEVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct NoNet;
impl Net for NoNet {
    fn call(&mut self, _: &Req) -> lbapi::Reply {
        lbapi::Reply::Offline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lbapi::fakes::*;
    use crate::musicbrainz::{MbAlias, MbArtist};

    const GROUP: &str = "1d98b0bc-5832-49d2-a93e-463032631a2f";

    fn rel(title: &str, artist: &str, mbids: &[&str], date: &str, kind: &str) -> serde_json::Value {
        serde_json::json!({
            "release_name": title, "artist_credit_name": artist, "artist_mbids": mbids, "release_date": date,
            "release_group_primary_type": kind, "release_mbid": "bf9e91ea-8029-4a04-a26a-224e00a83266", "release_group_mbid": GROUP,
        })
    }

    fn who() -> Identity {
        // The library's artists: 米津玄師 / Kenshi Yonezu (one artist, with a MusicBrainz id), Lumen (no id yet).
        let input = crate::identity::Input {
            names: vec![("米津玄師".into(), 5), ("Kenshi Yonezu".into(), 3), ("Lumen".into(), 4)],
            learned: vec![("米津玄師".into(), "Kenshi Yonezu".into())],
            mb: HashMap::from([
                (
                    crate::identity::fold("米津玄師"),
                    MbArtist { mbid: "6a1d0a19-4a4c-4d0d-9b40-000000000001".into(), name: "米津玄師".into(), sort_name: "Yonezu, Kenshi".into(), aliases: vec![MbAlias { name: "Kenshi Yonezu".into(), ..Default::default() }], ..Default::default() },
                ),
                (crate::identity::fold("Lumen"), MbArtist { mbid: "db92a151-1ac2-438b-bc43-b82e149ddd50".into(), name: "Lumen".into(), sort_name: "Lumen".into(), ..Default::default() }),
            ]),
            ..Default::default()
        };
        Identity::build(&input, 1)
    }

    fn body(items: Vec<serde_json::Value>) -> String {
        serde_json::Value::Array(items).to_string()
    }

    fn all() -> Vec<serde_json::Value> {
        vec![
            rel("Prism II", "Lumen", &["db92a151-1ac2-438b-bc43-b82e149ddd50"], "2026-11-10", "Album"),
            rel("Glass (Single)", "Lumen", &[], "2026-11-12", "Single"),
            // By id alone: the credit is written in a way the library doesn't have.
            rel("Spinel", "Yonezu Kenshi", &["6a1d0a19-4a4c-4d0d-9b40-000000000001"], "2026-11-01", "EP"),
            // The same name as a library artist's, but another artist: their id differs.
            rel("Other Lumen", "Lumen", &["ffffffff-ffff-4fff-8fff-ffffffffffff"], "2026-11-05", "Album"),
            rel("Stranger", "Nobody We Own", &[], "2026-11-09", "Album"),
            rel("Evenings", "Lumen", &[], "2026-11-02", "Album"), // the library has it
            rel("Prism II", "Lumen", &[], "2026-11-10", "Album"), // another edition of the first
            rel("Mix", "Various Artists", &[], "2026-11-03", "Album"),
        ]
    }

    #[test]
    fn releases_are_matched_to_the_library_artists_and_owned_albums_are_left_out() {
        let who = who();
        let albums = vec![("Evenings (Deluxe)".to_string(), "Lumen".to_string())];
        let lib = Library { who: &who, albums: &albums };
        let fresh = lbapi::parse_fresh(&body(all())).unwrap();
        let picked = select(&fresh, &lib);
        let titles: Vec<&str> = picked.iter().map(|r| r.title.as_str()).collect();
        // Newest first; the id-matched one is there; the other artist of the same name, strangers, the
        // various-artists one and the duplicate edition are not.
        assert_eq!(titles, ["Glass (Single)", "Prism II", "Evenings", "Spinel"]);
        // The library's album is left out when shown (it may arrive after the fetch).
        let cache = Cache { fetched: 1_800_000_000, attempted: 1_800_000_000, days: DAYS, items: select(&fresh, &Library { who: &who, albums: &[] }) };
        assert!(cache.items.iter().any(|r| r.title == "Evenings"));
        let v = view(&cache, &lib, true, 1_800_000_000, String::new());
        assert!(v.items.iter().all(|r| r.title != "Evenings"));
        assert_eq!(v.items[0].kind, "Single");
        assert!(v.items[0].cover.as_deref().unwrap().contains(GROUP));
    }

    #[test]
    fn upcoming_releases_are_marked() {
        let who = who();
        let lib = Library { who: &who, albums: &[] };
        let fresh = lbapi::parse_fresh(&body(vec![rel("Soon", "Lumen", &[], "2026-12-25", "Album"), rel("Out", "Lumen", &[], "2026-11-01", "Album")])).unwrap();
        let cache = Cache { fetched: 1, attempted: 1, days: DAYS, items: select(&fresh, &lib) };
        let now = 1_794_700_800; // 2026-11-15
        let v = view(&cache, &lib, true, now, String::new());
        assert_eq!(date_of(now), "2026-11-15");
        assert_eq!(v.items.iter().map(|r| (r.title.as_str(), r.upcoming)).collect::<Vec<_>>(), [("Soon", true), ("Out", false)]);
    }

    #[test]
    fn it_asks_once_a_day_only_when_allowed_and_keeps_the_answer_on_disk() {
        let dir = std::env::temp_dir().join(format!("cadence-releases-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join(FILE);
        let who = who();
        let lib = Library { who: &who, albums: &[] };
        let day = 86_400;
        let t0 = 1_794_787_200;
        let (mut net, mut clock) = fakes(vec![ok(&body(all())), ok(&body(all())), Reply::Offline, ok(&body(all()))]);
        use crate::lbapi::Reply;
        // Off: nothing is asked, nothing shown.
        let v = load(&file, t0, false, true, &mut net, &mut clock, &lib);
        assert!(net.asked.is_empty() && v.items.is_empty() && !v.enabled);
        // On, but only the kept releases are wanted (no window asking): nothing is asked.
        load(&file, t0, true, false, &mut net, &mut clock, &lib);
        assert!(net.asked.is_empty());
        // Opening Discover: asked, and the path has no account in it.
        let v = load(&file, t0, true, true, &mut net, &mut clock, &lib);
        assert_eq!(net.asked.len(), 1);
        assert!(net.asked[0].0.starts_with("/explore/fresh-releases/?days=30"));
        assert_eq!(v.items.len(), 4);
        assert_eq!(v.fetched, t0);
        // Again within the day (even after a restart: it's on disk): not asked.
        let v = load(&file, t0 + day - 10, true, true, &mut net, &mut clock, &lib);
        assert_eq!((net.asked.len(), v.items.len()), (1, 4));
        // A day later: asked again. Offline then: the kept releases stay, the reason is given, and
        // it isn't tried again at once.
        load(&file, t0 + day + 5, true, true, &mut net, &mut clock, &lib);
        assert_eq!(net.asked.len(), 2);
        let v = load(&file, t0 + 2 * day + 10, true, true, &mut net, &mut clock, &lib);
        assert_eq!(net.asked.len(), 3);
        assert!(!v.error.is_empty() && v.items.len() == 4, "{v:?}");
        let v = load(&file, t0 + 2 * day + 600, true, true, &mut net, &mut clock, &lib);
        assert_eq!(net.asked.len(), 3, "after a failure, an hour's rest");
        assert_eq!(v.items.len(), 4);
        // Turned off with releases kept: none are shown (their covers would still be loaded from
        // the Cover Art Archive), asked or not.
        for ask in [false, true] {
            let v = load(&file, t0 + 2 * day + 700, false, ask, &mut net, &mut clock, &lib);
            assert!(v.items.is_empty() && !v.enabled && v.fetched == 0, "{v:?}");
        }
        assert_eq!(net.asked.len(), 3);
        // An absurd time kept in the file doesn't overflow.
        std::fs::write(&file, r#"{"fetched":18446744073709551615,"attempted":18446744073709551615,"days":30,"items":[]}"#).unwrap();
        let _ = load(&file, t0, true, true, &mut net, &mut clock, &lib);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
