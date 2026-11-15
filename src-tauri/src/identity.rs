//! One artist across scripts: every artist name in the library maps to one artist, and names
//! known to be the same artist (米津玄師 / Kenshi Yonezu / よねづけんし, BTS / 방탄소년단) map to
//! the same one. Nothing here changes a file's tags.
//!
//! What says two names are one artist, strongest first:
//! 1. the user: pairs marked the same, and pairs marked different (artist-names.json). A
//!    "different" always wins: nothing below can join two names the user split;
//! 2. what the iTunes check learned (artist-names.json `latin`);
//! 3. MusicBrainz aliases (musicbrainz.rs; the same MusicBrainz artist, never two different ones);
//! 4. safe equivalences of the names themselves: case, accents, punctuation, spacing, full- and
//!    half-width forms (`fold`), and the reading of a name written only in kana or only in Hangul
//!    (りりあ。 ↔ Riria., 방탄소년단 ↔ Bangtan Sonyeondan). Kanji are never read for this (a
//!    dictionary's reading of a name is a guess; pinyin is the wrong reading for a Japanese name),
//!    and a name inside another ("Kenshi Yonezu Tribute Band") is not that artist.
//!
//! Built once per library load and whenever the decisions or the MusicBrainz cache change, and
//! shared as a cheap handle (`current()`): callers never rebuild it per song.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::{Mutex, RwLock};
use serde::Serialize;

use crate::ctx::Ctx;
use crate::musicbrainz::MbArtist;
use crate::romanize::{hangul_syllable, is_han, is_hangul, is_kana, kana_romaji};

/// A name reduced for comparing: full-width forms as their usual ones (ＡＢＣ → abc, ｶﾅ → カナ),
/// Latin letters without accents, letters and digits only, lower case. Kana keep their dakuten.
pub fn fold(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let mut out = String::with_capacity(s.len());
    for c in s.nfkc() {
        if (c as u32) < 0x250 {
            out.extend(std::iter::once(c).nfkd().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase));
        } else if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// The names in a credit ("A feat. B", "A & B", "A × B", "A, B"), as written: the splitter the
/// matching code shares (`autotag::split_credit`, with × as a joiner too). The artist pages use the
/// window's splitter (`splitCredit`, ported as `api::split_credit_like_ui`), which differs slightly
/// ("with", 、 and full-width ＆ only split here) and so stays separate.
pub fn split_names(credit: &str) -> Vec<String> {
    crate::autotag::split_credit(&credit.replace('×', "/"))
}

/// A Latin name joins a kana or Hangul name by reading alone only from this many letters on.
pub const MIN_READ_JOIN: usize = 4;

/// Long vowels written out or not: "toukyou", "tookyoo" and "tokyo" all become "tokyo".
pub fn collapse(romaji: &str) -> String {
    romaji.replace("ou", "o").replace("oo", "o").replace("uu", "u")
}

fn letters(s: &str) -> impl Iterator<Item = char> + '_ {
    s.chars().filter(|c| c.is_alphanumeric())
}

/// Written only in kana (punctuation and spaces aside): りりあ。, ヨルシカ.
pub fn kana_only(s: &str) -> bool {
    letters(s).next().is_some() && letters(s).all(|c| is_kana(c) || c == 'ー')
}

/// Written only in Hangul: 방탄소년단.
pub fn hangul_only(s: &str) -> bool {
    letters(s).next().is_some() && letters(s).all(is_hangul)
}

/// Any Chinese, Japanese or Korean letters.
pub fn cjk(s: &str) -> bool {
    s.chars().any(|c| is_han(c) || is_hangul(c) || is_kana(c))
}

/// Hangul letter by letter in Revised Romanization, other characters kept.
pub fn hangul_romaji(s: &str) -> String {
    s.chars().map(|c| hangul_syllable(c).unwrap_or_else(|| c.to_string())).collect()
}

/// The reading of a name written only in kana or only in Hangul, folded (long vowels kept);
/// None for any other name (kanji are never read for matching).
pub fn reading(s: &str) -> Option<String> {
    let r = if kana_only(s) {
        kana_romaji(s)
    } else if hangul_only(s) {
        hangul_romaji(s)
    } else {
        return None;
    };
    Some(fold(&r)).filter(|r| !r.is_empty())
}

/// Which language an artist sings in, as far as the names and MusicBrainz tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Japanese,
    Chinese,
    Korean,
}

/// One artist: every name the library writes them with, and what else they're known as.
#[derive(Clone, Debug, Default)]
pub struct Artist {
    /// The shared key (`primary_key`): the smallest folded library name.
    pub key: String,
    /// The library's spellings as written, the one on most songs first.
    pub names: Vec<(String, u32)>,
    /// Other names (MusicBrainz aliases, the catalogue's Latin name), as written, not in the library.
    pub aliases: Vec<String>,
    /// MusicBrainz search hints (misspellings and the like): for search only.
    pub hints: Vec<String>,
    pub mb: Option<MbArtist>,
    pub lang: Option<Lang>,
}

#[derive(Default, Debug)]
struct Inner {
    /// Folded name (library names and their aliases) → index into `artists`.
    by_key: HashMap<String, u32>,
    artists: Vec<Artist>,
    version: u64,
}

/// The artists of the library as one shared, read-only snapshot; cloning is cheap.
#[derive(Clone, Default, Debug)]
pub struct Identity(Arc<Inner>);

/// What a credit's name is: a known artist, or a name the identity doesn't have (folded).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Who {
    Known(u32),
    Loose(String),
}

/// Everything the identity is built from.
#[derive(Default, Clone)]
pub struct Input {
    /// Every artist name in the library as written, with its songs (each name of a credit, and the
    /// whole credits that are artist pages: "Simon & Garfunkel").
    pub names: Vec<(String, u32)>,
    pub same: Vec<(String, String)>,
    pub different: Vec<(String, String)>,
    pub learned: Vec<(String, String)>,
    /// MusicBrainz's artist for a library name, by the name's `fold`.
    pub mb: HashMap<String, MbArtist>,
}

struct Uf {
    parent: Vec<u32>,
    mbid: Vec<Option<String>>,
}

impl Uf {
    fn add(&mut self) -> u32 {
        self.parent.push(self.parent.len() as u32);
        self.mbid.push(None);
        (self.parent.len() - 1) as u32
    }
    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let p = self.parent[x as usize];
            self.parent[x as usize] = self.parent[p as usize];
            x = p;
        }
        x
    }
}

struct Builder {
    uf: Uf,
    nodes: HashMap<String, u32>,
    different: Vec<(u32, u32)>,
}

impl Builder {
    fn node(&mut self, key: &str) -> Option<u32> {
        if key.is_empty() {
            return None;
        }
        if let Some(&n) = self.nodes.get(key) {
            return Some(n);
        }
        let n = self.uf.add();
        self.nodes.insert(key.to_string(), n);
        Some(n)
    }

    /// Joins two names unless the user said they differ or they'd join two MusicBrainz artists.
    fn union(&mut self, a: u32, b: u32) -> bool {
        let (ra, rb) = (self.uf.find(a), self.uf.find(b));
        if ra == rb {
            return true;
        }
        for i in 0..self.different.len() {
            let (x, y) = self.different[i];
            let (fx, fy) = (self.uf.find(x), self.uf.find(y));
            if (fx == ra && fy == rb) || (fx == rb && fy == ra) {
                return false;
            }
        }
        let (ma, mb) = (self.uf.mbid[ra as usize].clone(), self.uf.mbid[rb as usize].clone());
        if let (Some(x), Some(y)) = (&ma, &mb)
            && x != y
        {
            return false;
        }
        self.uf.parent[rb as usize] = ra;
        self.uf.mbid[ra as usize] = ma.or(mb);
        true
    }

    /// Notes that a name is this MusicBrainz artist; false if its group already is another one.
    fn claim(&mut self, n: u32, mbid: &str) -> bool {
        let r = self.uf.find(n);
        match &self.uf.mbid[r as usize] {
            Some(m) => m == mbid,
            None => {
                self.uf.mbid[r as usize] = Some(mbid.to_string());
                true
            }
        }
    }
}

/// "Yonezu, Kenshi" → "Kenshi Yonezu".
pub fn unsort(sort_name: &str) -> Option<String> {
    let (last, first) = sort_name.split_once(", ")?;
    Some(format!("{} {}", first.trim(), last.trim()))
}

fn lang_of_country(c: &str) -> Option<Lang> {
    match c {
        "JP" => Some(Lang::Japanese),
        "KR" => Some(Lang::Korean),
        "CN" | "TW" | "HK" | "MO" | "SG" => Some(Lang::Chinese),
        _ => None,
    }
}

impl Identity {
    /// An empty one (names then only match by `fold` and reading).
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the identity (a few milliseconds for a library of thousands of artists).
    pub fn build(input: &Input, version: u64) -> Identity {
        let mut b = Builder { uf: Uf { parent: Vec::new(), mbid: Vec::new() }, nodes: HashMap::new(), different: Vec::new() };
        // The library's names, by folded key.
        let mut spellings: HashMap<u32, Vec<(String, u32)>> = HashMap::new();
        for (name, songs) in &input.names {
            let Some(n) = b.node(&fold(name)) else { continue };
            let list = spellings.entry(n).or_default();
            match list.iter_mut().find(|(s, _)| s == name) {
                Some(e) => e.1 += songs,
                None => list.push((name.clone(), *songs)),
            }
        }
        for (x, y) in &input.different {
            if let (Some(a), Some(c)) = (b.node(&fold(x)), b.node(&fold(y)))
                && a != c
            {
                b.different.push((a, c));
            }
        }
        let join = |b: &mut Builder, x: &str, y: &str| {
            if let (Some(a), Some(c)) = (b.node(&fold(x)), b.node(&fold(y))) {
                b.union(a, c);
            }
        };
        // 1. The user's own decisions.
        for (x, y) in &input.same {
            join(&mut b, x, y);
        }
        // 2. What the catalogue check learned.
        for (x, y) in &input.learned {
            join(&mut b, x, y);
        }
        // 3. MusicBrainz: each library name with the names of its MusicBrainz artist.
        let mut mb_of: HashMap<u32, &MbArtist> = HashMap::new();
        let lib_nodes: Vec<u32> = {
            let mut v: Vec<u32> = spellings.keys().copied().collect();
            v.sort_unstable();
            v
        };
        let key_of: HashMap<u32, String> = b.nodes.iter().map(|(k, &n)| (n, k.clone())).collect();
        for &n in &lib_nodes {
            if let Some(m) = input.mb.get(&key_of[&n]) {
                mb_of.insert(n, m);
            }
        }
        for &n in &lib_nodes {
            let Some(m) = mb_of.get(&n).copied() else { continue };
            if !b.claim(n, &m.mbid) {
                continue;
            }
            for other in m.names(false) {
                let Some(o) = b.node(&fold(&other)) else { continue };
                // Another library name that is a different MusicBrainz artist stays apart.
                if mb_of.get(&o).is_some_and(|om| om.mbid != m.mbid) {
                    continue;
                }
                b.union(n, o);
            }
        }
        // 4. Readings of names written only in kana or Hangul, against the library's Latin names.
        let mut by_reading: HashMap<String, Vec<u32>> = HashMap::new();
        let mut by_collapsed: HashMap<String, Vec<u32>> = HashMap::new();
        for &n in &lib_nodes {
            for (s, _) in &spellings[&n] {
                if let Some(r) = reading(s) {
                    by_collapsed.entry(collapse(&r)).or_default().push(n);
                    by_reading.entry(r).or_default().push(n);
                    break;
                }
            }
        }
        for list in by_reading.values() {
            for w in list.windows(2) {
                b.union(w[0], w[1]);
            }
        }
        for &n in &lib_nodes {
            let written = &spellings[&n];
            if written.iter().any(|(s, _)| cjk(s)) {
                continue;
            }
            let f = &key_of[&n];
            // Only a name long enough to say something ("Ko" is こう and こ and many more), and
            // only when it reads as exactly one artist: a short or ambiguous name joins only
            // through what is known (the user, a learned pair, MusicBrainz).
            if f.chars().count() < MIN_READ_JOIN {
                continue;
            }
            let hits: Vec<u32> = by_reading.get(f).into_iter().chain(by_collapsed.get(&collapse(f))).flatten().copied().collect();
            let mut roots: Vec<u32> = hits.iter().map(|&h| b.uf.find(h)).collect();
            roots.sort_unstable();
            roots.dedup();
            if let [_] = roots.as_slice() {
                b.union(n, hits[0]);
            }
        }

        // The groups with a library name in them.
        let mut groups: HashMap<u32, Vec<u32>> = HashMap::new();
        for &n in &lib_nodes {
            let r = b.uf.find(n);
            groups.entry(r).or_default().push(n);
        }
        let mut roots: Vec<(u32, Vec<u32>)> = groups.into_iter().collect();
        roots.sort_by_key(|(_, ns)| ns[0]);
        let mut artists: Vec<Artist> = Vec::with_capacity(roots.len());
        let mut root_index: HashMap<u32, u32> = HashMap::new();
        for (root, ns) in roots {
            let mut names: Vec<(String, u32)> = ns.iter().flat_map(|n| spellings[n].iter().cloned()).collect();
            names.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let key = ns.iter().map(|n| key_of[n].as_str()).min().unwrap_or("").to_string();
            let mb = ns.iter().find_map(|n| mb_of.get(n).copied()).filter(|m| b.uf.mbid[root as usize].as_deref() == Some(m.mbid.as_str())).cloned();
            let lang = mb
                .as_ref()
                .and_then(|m| m.country.as_deref().and_then(lang_of_country))
                .or_else(|| names.iter().any(|(s, _)| s.chars().any(is_kana)).then_some(Lang::Japanese))
                .or_else(|| names.iter().any(|(s, _)| s.chars().any(is_hangul)).then_some(Lang::Korean));
            root_index.insert(root, artists.len() as u32);
            artists.push(Artist { key, names, aliases: Vec::new(), hints: Vec::new(), mb, lang });
        }
        let mut by_key: HashMap<String, u32> = HashMap::with_capacity(b.nodes.len());
        let all: Vec<(String, u32)> = b.nodes.iter().map(|(k, &n)| (k.clone(), n)).collect();
        for (k, n) in all {
            let r = b.uf.find(n);
            if let Some(&i) = root_index.get(&r) {
                by_key.insert(k, i);
            }
        }
        // Aliases as written, for search and the artist page.
        let add_alias = |a: &mut Artist, name: &str, hint: bool| {
            let f = fold(name);
            if f.is_empty() || a.names.iter().any(|(s, _)| fold(s) == f) || a.aliases.iter().chain(&a.hints).any(|x| fold(x) == f) {
                return;
            }
            if hint { a.hints.push(name.to_string()) } else { a.aliases.push(name.to_string()) }
        };
        for (x, y) in input.learned.iter().chain(&input.same) {
            for (from, to) in [(x, y), (y, x)] {
                if let Some(&i) = by_key.get(&fold(from))
                    && by_key.get(&fold(to)) == Some(&i)
                {
                    add_alias(&mut artists[i as usize], to, false);
                }
            }
        }
        for a in &mut artists {
            if let Some(m) = a.mb.clone() {
                for n in m.names(false) {
                    add_alias(a, &n, false);
                }
                for n in m.hints() {
                    add_alias(a, &n, true);
                }
            }
        }
        Identity(Arc::new(Inner { by_key, artists, version }))
    }

    /// For tests: the library names of the pairs, each pair learned as the same artist.
    #[cfg(test)]
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Identity {
        let names = pairs.iter().flat_map(|(a, b)| [(a.to_string(), 1), (b.to_string(), 1)]).collect();
        let learned = pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        Identity::build(&Input { names, learned, ..Default::default() }, 1)
    }

    pub fn version(&self) -> u64 {
        self.0.version
    }

    pub fn artists(&self) -> &[Artist] {
        &self.0.artists
    }

    pub fn artist(&self, i: u32) -> &Artist {
        &self.0.artists[i as usize]
    }

    /// The artist a single name is (as written, or any spelling or alias of it).
    pub fn of_name(&self, name: &str) -> Option<u32> {
        self.0.by_key.get(&fold(name)).copied()
    }

    /// Who a credit names: the whole credit when it's an artist ("Simon & Garfunkel"), else each
    /// of its names ("A feat. B" → A, B), in order, without repeats.
    pub fn credit(&self, credit: &str) -> Vec<Who> {
        if let Some(i) = self.of_name(credit) {
            return vec![Who::Known(i)];
        }
        let mut out: Vec<Who> = Vec::new();
        for n in split_names(credit) {
            let who = match self.of_name(&n) {
                Some(i) => Who::Known(i),
                None => {
                    let f = fold(&n);
                    if f.is_empty() {
                        continue;
                    }
                    Who::Loose(f)
                }
            };
            if !out.contains(&who) {
                out.push(who);
            }
        }
        if out.is_empty() {
            let f = fold(credit);
            if !f.is_empty() {
                out.push(Who::Loose(f));
            }
        }
        out
    }

    /// The key of whoever leads a credit ("A feat. B" is A's), the same for every spelling of an
    /// artist: for grouping songs by artist (shuffle, mixes, autoplay).
    pub fn primary_key(&self, credit: &str) -> String {
        match self.credit(credit).into_iter().next() {
            Some(Who::Known(i)) => self.0.artists[i as usize].key.clone(),
            Some(Who::Loose(f)) => f,
            None => String::new(),
        }
    }

    /// Whether two credits share an artist ("Kenshi Yonezu" and "米津玄師 feat. DAOKO"). Names
    /// the identity doesn't have are compared by `fold`, and by reading when one is in kana or
    /// Hangul; a name inside another is not a match.
    pub fn same_artist(&self, a: &str, b: &str) -> bool {
        let (fa, fb) = (fold(a), fold(b));
        if !fa.is_empty() && fa == fb {
            return true;
        }
        let tokens = |credit: &str| -> Vec<String> {
            let mut out = Vec::new();
            let mut add = |w: Who, raw: &str| {
                match w {
                    Who::Known(i) => out.push(format!("#{i}")),
                    Who::Loose(f) => {
                        out.push(format!("f:{f}"));
                        match reading(raw) {
                            Some(r) => out.push(format!("r:{}", collapse(&r))),
                            None if !cjk(raw) => out.push(format!("l:{}", collapse(&f))),
                            None => {}
                        }
                    }
                }
            };
            match self.credit(credit).as_slice() {
                [Who::Known(i)] if self.of_name(credit).is_some() => add(Who::Known(*i), credit),
                _ => {
                    for n in split_names(credit) {
                        let w = match self.of_name(&n) {
                            Some(i) => Who::Known(i),
                            None => Who::Loose(fold(&n)),
                        };
                        if !matches!(&w, Who::Loose(f) if f.is_empty()) {
                            add(w, &n);
                        }
                    }
                }
            }
            out
        };
        let (x, y) = (tokens(a), tokens(b));
        x.iter().any(|t| {
            y.iter().any(|u| {
                // The same artist or folded name (a Latin name's collapsed form only counts
                // against a kana or Hangul name's reading, so "Moon" isn't "Mon").
                let same = t == u && !t.starts_with("l:");
                // (Not for a short name: "Ko" is too many kana names.)
                let read = ((t.starts_with("r:") && u.starts_with("l:")) || (t.starts_with("l:") && u.starts_with("r:")))
                    && t.get(2..) == u.get(2..)
                    && t.get(2..).is_some_and(|s| s.chars().count() >= MIN_READ_JOIN);
                same || read
            })
        })
    }

    /// The artist pages merged by artist: each page's lower-case name → the page shown for it (of
    /// a merged artist's pages, the one with most songs). `pages`: each page with its songs.
    pub fn merge_pages(&self, pages: &[(String, u32)]) -> HashMap<String, String> {
        let mut groups: HashMap<u32, Vec<&(String, u32)>> = HashMap::new();
        let mut out = HashMap::new();
        for p in pages {
            match self.of_name(&p.0) {
                Some(i) => groups.entry(i).or_default().push(p),
                None => {
                    out.insert(p.0.to_lowercase(), p.0.clone());
                }
            }
        }
        for (i, list) in groups {
            let names = &self.0.artists[i as usize].names;
            let rank = |n: &str| names.iter().position(|(s, _)| s.to_lowercase() == n.to_lowercase()).unwrap_or(usize::MAX);
            let best = list
                .iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| rank(&b.0).cmp(&rank(&a.0))).then_with(|| b.0.cmp(&a.0)))
                .map(|p| p.0.clone())
                .unwrap_or_default();
            for p in list {
                out.insert(p.0.to_lowercase(), best.clone());
            }
        }
        out
    }
}

// ---------- the shared snapshot ----------

static CURRENT: RwLock<Option<Identity>> = RwLock::new(None);
/// Bumped when what the identity is built from changes (the library, decisions, MusicBrainz), and
/// at the start of every build (each build's identity has a version of its own, so caches stamped
/// with the version they were built from know when they are stale).
static WANTED: AtomicU64 = AtomicU64::new(1);
/// The version of the newest build stored.
static BUILT: AtomicU64 = AtomicU64::new(0);
static REBUILDING: AtomicBool = AtomicBool::new(false);
/// One build at a time (the background rebuild, a decision's rebuild, startup).
static BUILD: Mutex<()> = Mutex::new(());
static APP: OnceLock<Ctx> = OnceLock::new();
/// What the window was last told (a fingerprint of the merged artists), so it's told only of changes.
static TOLD: Mutex<u64> = Mutex::new(0);

#[cfg(test)]
thread_local! {
    static TEST: std::cell::RefCell<Option<Identity>> = const { std::cell::RefCell::new(None) };
}

/// For tests: this thread's identity (other tests run on other threads).
#[cfg(test)]
pub fn set_for_test(id: Option<Identity>) {
    TEST.with(|t| *t.borrow_mut() = id);
}

/// The latest change asked for (above `Identity::version` while a rebuild is pending).
pub fn wanted() -> u64 {
    WANTED.load(Ordering::SeqCst)
}

/// The identity as last built (empty before the first build: names then only match by `fold`).
pub fn current() -> Identity {
    #[cfg(test)]
    if let Some(id) = TEST.with(|t| t.borrow().clone()) {
        return id;
    }
    CURRENT.read().clone().unwrap_or_default()
}

/// The library or a decision changed: the identity is built again in the background (several
/// changes in a row are one rebuild). Before `init` (tests, startup) it's only noted.
pub fn invalidate() {
    WANTED.fetch_add(1, Ordering::SeqCst);
    let Some(ctx) = APP.get() else { return };
    if REBUILDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new().name("cadence-identity".into()).spawn(move || {
        crate::jobs::background_priority();
        run_pending(&REBUILDING, pending, || {
            rebuild_now(&ctx);
        });
    });
    if spawned.is_err() {
        REBUILDING.store(false, Ordering::SeqCst);
    }
}

/// Builds while a change is pending; after letting go of the "rebuilding" flag, looks once more,
/// so a change asked for just as the loop ended (when `invalidate` saw the flag still set and left
/// it to this thread) isn't lost.
fn run_pending(flag: &AtomicBool, pending: impl Fn() -> bool, mut build: impl FnMut()) {
    loop {
        while pending() {
            build();
        }
        flag.store(false, Ordering::SeqCst);
        if !pending() || flag.swap(true, Ordering::SeqCst) {
            return;
        }
    }
}

/// A change was asked for that no stored build includes yet.
fn pending() -> bool {
    BUILT.load(Ordering::SeqCst) < WANTED.load(Ordering::SeqCst)
}

/// What the identity is built from was replaced (a restore): built again now, on the calling
/// thread, when the app keeps one (`init` ran); otherwise only noted, as `invalidate` does.
pub fn rebuild_after_change(ctx: &Ctx) {
    if APP.get().is_some() {
        rebuild_now(ctx);
    } else {
        WANTED.fetch_add(1, Ordering::SeqCst);
    }
}

/// Starts keeping the identity up to date (once, at startup), and builds it.
pub fn init(ctx: &Ctx) {
    let _ = APP.set(ctx.clone());
    rebuild_now(ctx);
}

/// What the identity is built from, read from the app's state (the library's lock is let go
/// before anything else is read).
pub fn gather(state: &crate::state::AppState) -> Input {
    let mut counts: HashMap<String, u32> = HashMap::new();
    {
        let lib = state.library.lock();
        for t in &lib.snapshot.tracks {
            if crate::autotag::untagged(t) {
                continue;
            }
            let mut seen: HashSet<String> = HashSet::new();
            for n in split_names(&t.artist).into_iter().chain(split_names(&t.album_artist)) {
                if seen.insert(n.clone()) {
                    *counts.entry(n).or_default() += 1;
                }
            }
        }
        // Whole credits that are artist pages ("Simon & Garfunkel"): a credit is matched whole first.
        let index = crate::api::artist_index(lib.snapshot.albums.iter().map(|a| a.artist.as_str()));
        for a in &lib.snapshot.albums {
            if let Some(names) = index.get(&a.artist) {
                for n in names {
                    if split_names(n).len() > 1 {
                        *counts.entry(n.clone()).or_default() += a.track_ids.len() as u32;
                    }
                }
            }
        }
    }
    let d = crate::libfix::decisions(&state.data_dir);
    Input {
        names: counts.into_iter().collect(),
        same: d.same,
        different: d.different,
        learned: d.learned,
        mb: crate::musicbrainz::accepted(&state.data_dir),
    }
}

/// Builds the identity now (on the calling thread) and makes it the current one. The window is
/// told ("artist-identity-changed") when the merged artists changed.
pub fn rebuild_now(ctx: &Ctx) -> Identity {
    let started = std::time::Instant::now();
    let (id, names) = build_and_store(|| gather(&ctx.state));
    log::info!("artists: {} for {names} names in {:?}", id.artists().len(), started.elapsed());
    let print = fingerprint(&id);
    let changed = {
        let mut told = TOLD.lock();
        let changed = *told != print;
        *told = print;
        changed
    };
    if changed {
        ctx.emit("artist-identity-changed", id.version());
    }
    id
}

/// One build, serialized with every other: a version of its own taken at the start (so whatever
/// changed before it is in it, and it's newer than every earlier build), stored only if nothing
/// newer is stored already. Returns the identity now current and how many names it was built from.
fn build_and_store(gather: impl FnOnce() -> Input) -> (Identity, usize) {
    let _one = BUILD.lock();
    let version = WANTED.fetch_add(1, Ordering::SeqCst) + 1;
    let input = gather();
    let id = Identity::build(&input, version);
    let stored = {
        let mut cur = CURRENT.write();
        match cur.as_ref() {
            Some(c) if c.version() > version => c.clone(),
            _ => {
                *cur = Some(id.clone());
                id
            }
        }
    };
    BUILT.fetch_max(version, Ordering::SeqCst);
    (stored, input.names.len())
}

fn fingerprint(id: &Identity) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for g in view(id).groups {
        g.names.hash(&mut h);
        g.aliases.hash(&mut h);
    }
    h.finish()
}

// ---------- for the window ----------

/// An artist with more than one spelling in the library (or other names), for the window.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    /// The library's spellings, the one on most songs first.
    pub names: Vec<String>,
    /// Other names it's known by (not in the library).
    pub aliases: Vec<String>,
    pub mbid: Option<String>,
    /// MusicBrainz's sort name ("Yonezu, Kenshi").
    pub sort: Option<String>,
    /// MusicBrainz's name in the artist's own script, and in Latin letters.
    pub native: Option<String>,
    pub latin: Option<String>,
    pub lang: Option<Lang>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct IdentityView {
    pub version: u64,
    /// Only the artists the library writes in more than one way.
    pub groups: Vec<GroupView>,
}

pub fn view(id: &Identity) -> IdentityView {
    let groups = id
        .artists()
        .iter()
        .filter(|a| a.names.iter().map(|(n, _)| n.to_lowercase()).collect::<HashSet<_>>().len() > 1)
        .map(|a| GroupView {
            names: a.names.iter().map(|(n, _)| n.clone()).collect(),
            aliases: a.aliases.clone(),
            mbid: a.mb.as_ref().map(|m| m.mbid.clone()),
            sort: a.mb.as_ref().map(|m| m.sort_name.clone()).filter(|s| !s.is_empty()),
            native: a.mb.as_ref().and_then(|m| m.native_and_latin().0),
            latin: a.mb.as_ref().and_then(|m| m.native_and_latin().1),
            lang: a.lang,
        })
        .collect();
    IdentityView { version: id.version(), groups }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::musicbrainz::{MbAlias, MbArtist};

    fn names(list: &[&str]) -> Vec<(String, u32)> {
        list.iter().map(|n| (n.to_string(), 1)).collect()
    }
    fn pair(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
    }
    fn yonezu() -> MbArtist {
        MbArtist {
            mbid: "mb-yonezu".into(),
            name: "米津玄師".into(),
            sort_name: "Yonezu, Kenshi".into(),
            country: Some("JP".into()),
            aliases: ["よねづけんし", "Kenshi Yonezu", "켄시 요네즈"]
                .iter()
                .map(|n| MbAlias { name: n.to_string(), ..Default::default() })
                .collect(),
        }
    }

    #[test]
    fn folding_is_safe() {
        assert_eq!(fold("ＢＴＳ"), "bts");
        assert_eq!(fold("Beyoncé"), fold("BEYONCE"));
        assert_eq!(fold("ｶﾞｷﾞ"), fold("ガギ"));
        assert_ne!(fold("か"), fold("が"));
        assert_eq!(fold("Kenshi  Yonezu!"), "kenshiyonezu");
    }

    #[test]
    fn learned_pairs_join_and_the_user_s_different_beats_everything() {
        let mut input = Input { names: names(&["米津玄師", "Kenshi Yonezu", "YOASOBI"]), learned: vec![pair("米津玄師", "Kenshi Yonezu")], ..Default::default() };
        let id = Identity::build(&input, 1);
        assert_eq!(id.of_name("米津玄師"), id.of_name("kenshi yonezu"));
        assert_ne!(id.of_name("YOASOBI"), id.of_name("米津玄師"));
        // MusicBrainz agrees, the catalogue agrees, the user doesn't: the user wins.
        input.mb.insert(fold("米津玄師"), yonezu());
        input.same.push(pair("米津玄師", "Kenshi Yonezu"));
        input.different.push(pair("Kenshi Yonezu", "米津玄師"));
        let id = Identity::build(&input, 2);
        assert_ne!(id.of_name("米津玄師"), id.of_name("Kenshi Yonezu"));
        // Not through a third name either (よねづけんし, the MusicBrainz alias, joins one side only).
        input.names.push(("よねづけんし".into(), 1));
        let id = Identity::build(&input, 3);
        assert_ne!(id.of_name("米津玄師"), id.of_name("Kenshi Yonezu"));
        assert!(!id.same_artist("米津玄師", "Kenshi Yonezu"));
    }

    #[test]
    fn user_same_joins_what_nothing_else_knows() {
        let input = Input { names: names(&["ヨルシカ", "Yorushika (band)"]), same: vec![pair("ヨルシカ", "Yorushika (band)")], ..Default::default() };
        let id = Identity::build(&input, 1);
        assert!(id.same_artist("ヨルシカ", "Yorushika (band)"));
    }

    #[test]
    fn musicbrainz_aliases_join_spellings() {
        let mut input = Input { names: names(&["米津玄師", "Kenshi Yonezu", "켄시 요네즈", "Someone"]), ..Default::default() };
        input.mb.insert(fold("米津玄師"), yonezu());
        let id = Identity::build(&input, 1);
        let k = id.of_name("米津玄師").unwrap();
        assert_eq!(id.of_name("Kenshi Yonezu"), Some(k));
        assert_eq!(id.of_name("켄시 요네즈"), Some(k));
        assert_eq!(id.of_name("よねづけんし"), Some(k), "an alias finds the artist too");
        assert_ne!(id.of_name("Someone"), Some(k));
        assert_eq!(id.primary_key("Kenshi Yonezu feat. Hatsune Miku"), id.primary_key("米津玄師"));
        let a = id.artist(k);
        assert_eq!(a.lang, Some(Lang::Japanese));
        assert!(a.aliases.iter().any(|x| x == "よねづけんし"));
    }

    #[test]
    fn two_musicbrainz_artists_are_never_joined() {
        // Both list "Yuki" as an alias; the library has a "Yuki" too.
        let mk = |id: &str, name: &str| MbArtist {
            mbid: id.into(),
            name: name.into(),
            sort_name: name.into(),
            country: None,
            aliases: vec![MbAlias { name: "Yuki".into(), ..Default::default() }],
        };
        let mut input = Input { names: names(&["Yuki Isoya", "Yuki Kajiura", "Yuki"]), ..Default::default() };
        input.mb.insert(fold("Yuki Isoya"), mk("a", "Yuki Isoya"));
        input.mb.insert(fold("Yuki Kajiura"), mk("b", "Yuki Kajiura"));
        let id = Identity::build(&input, 1);
        assert_ne!(id.of_name("Yuki Isoya"), id.of_name("Yuki Kajiura"));
    }

    #[test]
    fn kana_and_hangul_readings_are_safe_equivalences() {
        let id = Identity::build(&Input { names: names(&["りりあ。", "Riria.", "とうきょう", "Tokyo", "방탄소년단", "Bangtan Sonyeondan", "ヨネヅ", "よねづ"]), ..Default::default() }, 1);
        assert_eq!(id.of_name("りりあ。"), id.of_name("Riria."));
        assert_eq!(id.of_name("とうきょう"), id.of_name("Tokyo"), "long vowels written out or not");
        assert_eq!(id.of_name("방탄소년단"), id.of_name("Bangtan Sonyeondan"));
        assert_eq!(id.of_name("ヨネヅ"), id.of_name("よねづ"), "hiragana and katakana");
        // And without the identity knowing the names.
        let empty = Identity::new();
        assert!(empty.same_artist("りりあ。", "Riria."));
        assert!(empty.same_artist("방탄소년단", "Bangtan Sonyeondan"));
        // Latin names are not collapsed against each other.
        assert!(!empty.same_artist("Moon", "Mon"));
    }

    #[test]
    fn kanji_are_never_read_and_containment_is_no_match() {
        let id = Identity::build(&Input { names: names(&["米津玄師", "Yonezu Kenshi", "Mi Jin Xuan Shi", "Kenshi Yonezu", "Kenshi Yonezu Tribute Band"]), ..Default::default() }, 1);
        assert_ne!(id.of_name("米津玄師"), id.of_name("Yonezu Kenshi"), "no dictionary reading of kanji");
        assert_ne!(id.of_name("米津玄師"), id.of_name("Mi Jin Xuan Shi"), "no pinyin of a Japanese name");
        assert_ne!(id.of_name("Kenshi Yonezu"), id.of_name("Kenshi Yonezu Tribute Band"));
        assert!(!id.same_artist("Kenshi Yonezu", "Kenshi Yonezu Tribute Band"));
        assert!(!Identity::new().same_artist("東京", "Tokyo"));
    }

    #[test]
    fn credits_with_separators_inside_names_are_matched_whole_first() {
        let id = Identity::build(&Input { names: names(&["Simon & Garfunkel", "Simon", "Garfunkel", "Tyler, the Creator", "Earth, Wind & Fire", "AC/DC"]), ..Default::default() }, 1);
        let one = |c: &str| id.credit(c);
        assert_eq!(one("Simon & Garfunkel").len(), 1);
        assert_eq!(one("Tyler, the Creator").len(), 1);
        assert_eq!(one("Earth, Wind & Fire").len(), 1);
        assert_eq!(one("AC/DC").len(), 1);
        assert_ne!(id.primary_key("Simon & Garfunkel"), id.primary_key("Simon"));
        // "feat." credits: the lead artist, and both names count as shared.
        let id = Identity::from_pairs(&[("米津玄師", "Kenshi Yonezu")]);
        assert_eq!(id.primary_key("Kenshi Yonezu feat. DAOKO"), id.primary_key("米津玄師"));
        assert_eq!(id.primary_key("米津玄師 × 菅田将暉"), id.primary_key("Kenshi Yonezu"));
        assert!(id.same_artist("DAOKO feat. 米津玄師", "Kenshi Yonezu"));
        assert!(id.same_artist("Hikaru Utada & 米津玄師", "Kenshi Yonezu"));
        assert!(!id.same_artist("Kenshi Yonezu", "Someone Else"));
    }

    #[test]
    fn short_or_ambiguous_latin_names_do_not_join_by_reading() {
        // "Ko" reads as こう (collapsed) and こ: it joins neither, nor do they join each other.
        let id = Identity::build(&Input { names: names(&["Ko", "こう", "こ"]), ..Default::default() }, 1);
        assert_ne!(id.of_name("Ko"), id.of_name("こう"));
        assert_ne!(id.of_name("Ko"), id.of_name("こ"));
        assert_ne!(id.of_name("こう"), id.of_name("こ"));
        assert!(!Identity::new().same_artist("Ko", "こう"));
        // Long enough but two different kana artists read the same once collapsed: no join.
        let id = Identity::build(&Input { names: names(&["Yuki", "ゆうき", "ゆき"]), ..Default::default() }, 1);
        assert_ne!(id.of_name("Yuki"), id.of_name("ゆうき"));
        assert_ne!(id.of_name("Yuki"), id.of_name("ゆき"));
        // One counterpart (two spellings of it are one artist already): joined.
        let id = Identity::build(&Input { names: names(&["Riria.", "りりあ。", "リリア"]), ..Default::default() }, 1);
        assert_eq!(id.of_name("Riria."), id.of_name("りりあ。"));
        assert_eq!(id.of_name("Riria."), id.of_name("リリア"));
        // A short name still joins through what is known.
        let id = Identity::build(&Input { names: names(&["Ko", "こう"]), same: vec![pair("Ko", "こう")], ..Default::default() }, 1);
        assert_eq!(id.of_name("Ko"), id.of_name("こう"));
    }

    #[test]
    fn every_build_has_a_newer_version_and_the_newest_stays() {
        let (a, _) = build_and_store(Input::default);
        let (b, _) = build_and_store(Input::default);
        assert!(b.version() > a.version(), "a rebuild after a decision is a new version");
        assert!(wanted() >= b.version());
        // Builds from several threads at once: one at a time, each newer, the newest kept.
        let versions: Vec<u64> = (0..6).map(|_| std::thread::spawn(|| build_and_store(Input::default).0.version())).collect::<Vec<_>>().into_iter().map(|h| h.join().unwrap()).collect();
        let distinct: HashSet<u64> = versions.iter().copied().collect();
        assert_eq!(distinct.len(), versions.len());
        let stored = CURRENT.read().as_ref().map(|i| i.version()).unwrap();
        assert!(stored >= *versions.iter().max().unwrap());
    }

    #[test]
    fn a_change_asked_for_as_the_rebuild_ends_is_not_lost() {
        let flag = AtomicBool::new(true);
        let (wanted, built) = (AtomicU64::new(2), AtomicU64::new(1));
        let first_false = AtomicBool::new(true);
        let pending = || {
            let p = built.load(Ordering::SeqCst) < wanted.load(Ordering::SeqCst);
            // Just as the loop sees nothing pending, another change comes: its `invalidate` finds
            // the flag still set and leaves the work to this thread.
            if !p && first_false.swap(false, Ordering::SeqCst) {
                wanted.fetch_add(1, Ordering::SeqCst);
                assert!(flag.load(Ordering::SeqCst));
            }
            p
        };
        let mut builds = 0;
        run_pending(&flag, pending, || {
            builds += 1;
            built.store(wanted.load(Ordering::SeqCst), Ordering::SeqCst);
        });
        assert_eq!(builds, 2, "the late change was built");
        assert_eq!(built.load(Ordering::SeqCst), 3);
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn merged_pages_show_the_spelling_on_most_songs() {
        let mut input = Input { names: vec![("米津玄師".into(), 3), ("Kenshi Yonezu".into(), 10), ("YOASOBI".into(), 4)], ..Default::default() };
        input.learned.push(pair("米津玄師", "Kenshi Yonezu"));
        let id = Identity::build(&input, 1);
        let shown = id.merge_pages(&[("米津玄師".into(), 3), ("Kenshi Yonezu".into(), 10), ("YOASOBI".into(), 4), ("Nobody".into(), 1)]);
        assert_eq!(shown["米津玄師"], "Kenshi Yonezu");
        assert_eq!(shown["kenshi yonezu"], "Kenshi Yonezu");
        assert_eq!(shown["yoasobi"], "YOASOBI");
        assert_eq!(shown["nobody"], "Nobody");
        let v = view(&id);
        assert_eq!(v.groups.len(), 1);
        assert_eq!(v.groups[0].names, ["Kenshi Yonezu", "米津玄師"]);
    }
}
