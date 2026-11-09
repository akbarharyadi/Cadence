//! What the listener told Needs Attention: problems to ignore ("it's how I want it"), and kinds
//! of problem not to show at all (missing years, say). Kept in `attention.json`.
//!
//! An ignored problem is a key the page makes: its kind and what it's about ("copies|<song id>",
//! "album|<album id>"). Only the page reads them; nothing else changes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attention {
    #[serde(default)]
    pub ignored: Vec<String>,
    /// Kinds of problem not shown.
    #[serde(default)]
    pub hidden: Vec<String>,
}

fn file(dir: &Path) -> PathBuf {
    dir.join("attention.json")
}

pub fn load(dir: &Path) -> Attention {
    std::fs::read(file(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(ctx: &Ctx, a: &Attention) {
    if let Ok(b) = serde_json::to_vec(a) {
        let _ = std::fs::write(file(&ctx.state.data_dir), b);
    }
    ctx.emit("attention-changed", ());
}

fn toggle(list: &mut Vec<String>, keys: Vec<String>, on: bool) {
    for k in keys {
        let has = list.contains(&k);
        if on && !has {
            list.push(k);
        } else if !on && has {
            list.retain(|x| x != &k);
        }
    }
}

pub fn ignore(ctx: &Ctx, keys: Vec<String>, ignored: bool) -> Attention {
    let mut a = load(&ctx.state.data_dir);
    toggle(&mut a.ignored, keys, ignored);
    save(ctx, &a);
    a
}

pub fn hide(ctx: &Ctx, kind: String, hidden: bool) -> Attention {
    let mut a = load(&ctx.state.data_dir);
    toggle(&mut a.hidden, vec![kind], hidden);
    save(ctx, &a);
    a
}
