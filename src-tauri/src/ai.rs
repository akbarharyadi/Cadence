//! The AI DJ: a chat model (Z.ai GLM by default, any OpenAI-compatible service works) picks songs
//! from your library ("Ask the DJ" and daily mixes), suggests new music and finds lyrics.
//!
//! The model never sees the whole library: Cadence sends a short list (at most `SHORTLIST` songs,
//! each with a few words about how it sounds from the local sound analysis) and a short summary
//! of your listening, then checks every id it gets back. With web search on, GLM can look things
//! up (new releases, lyrics); services that don't support it are asked again without.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::history::HistoryStats;
use crate::lanes::Pool;
use crate::library::Track;

pub const KEY_SECRET: &str = "ai-key";
const SHORTLIST: usize = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// An online OpenAI-compatible service (needs an API key).
    Cloud,
    /// Also what older settings with a provider that no longer exists (Ollama) become.
    #[default]
    #[serde(other)]
    Off,
}

/// Which online service answers: Anthropic's Claude, or any OpenAI-compatible one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    /// Claude, through Anthropic's Messages API (with its own web search).
    #[default]
    Claude,
    /// An OpenAI-compatible chat service (Z.ai GLM by default).
    #[serde(other)]
    Openai,
}

impl Service {
    /// Settings saved before Claude was offered were all for an OpenAI-compatible service.
    fn legacy() -> Self {
        Service::Openai
    }

    /// Each service keeps its own key (Windows Credential Manager / the keychain).
    pub fn key_name(self) -> &'static str {
        match self {
            Service::Claude => "ai-key-claude",
            Service::Openai => KEY_SECRET,
        }
    }
}

pub const CLAUDE_MODEL: &str = "claude-opus-5-5";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiSettings {
    pub provider: Provider,
    #[serde(default = "Service::legacy")]
    pub service: Service,
    pub cloud_url: String,
    pub cloud_model: String,
    pub claude_model: String,
    /// Let the model search the web (new music, lyrics).
    pub web_search: bool,
    /// "Ask the DJ": let the AI order and name the songs Cadence picked (a second, short request).
    #[serde(default = "yes")]
    pub dj_order: bool,
}

fn yes() -> bool {
    true
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            provider: Provider::Off,
            service: Service::Claude,
            web_search: true,
            // Z.ai's GLM Coding Plan (as the old Cadence used); any OpenAI-compatible service works.
            cloud_url: "https://api.z.ai/api/coding/paas/v4".into(),
            cloud_model: "glm-5.3-flash".into(),
            claude_model: CLAUDE_MODEL.into(),
            dj_order: true,
        }
    }
}

struct Endpoint {
    url: String,
    model: String,
    key: Option<String>,
}

fn endpoint(s: &AiSettings) -> Result<Endpoint, String> {
    match s.provider {
        Provider::Off => Err("The AI DJ is off. Turn it on in Settings.".into()),
        Provider::Cloud => {
            let key = crate::secrets::get(s.service.key_name());
            Ok(match s.service {
                Service::Claude => Endpoint {
                    url: "https://api.anthropic.com/v1/messages".into(),
                    model: if s.claude_model.trim().is_empty() { CLAUDE_MODEL.into() } else { s.claude_model.trim().to_string() },
                    key: Some(key.ok_or("Add your Anthropic API key in Settings to use Claude.")?),
                },
                Service::Openai => {
                    let url = s.cloud_url.trim().trim_end_matches('/').to_string();
                    check_url(&url)?;
                    // A server on this computer or the home network (Ollama, LM Studio) may need no key.
                    if key.is_none() && !is_local_url(&url) {
                        return Err("Add your API key in Settings to use the online AI DJ.".into());
                    }
                    Endpoint { url, model: s.cloud_model.clone(), key }
                }
            })
        }
    }
}

/// An AI server address the key may be sent to: https://, or plain http:// only to this computer
/// or the home network (where a local server like Ollama usually has no https).
pub fn check_url(url: &str) -> Result<(), String> {
    let u = url.trim().to_ascii_lowercase();
    if u.starts_with("https://") && host_of(&u).is_some_and(|h| !h.is_empty()) {
        return Ok(());
    }
    if u.starts_with("http://") {
        if is_local_url(&u) {
            return Ok(());
        }
        return Err("The AI server's address must start with https:// (plain http:// is only for a server on this computer or your home network): your API key would be sent unencrypted.".into());
    }
    Err("The AI server's address should look like https://api.example.com/v1".into())
}

/// The host of an http(s) address, without user name or port ("[::1]" → "::1").
fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(v6) = host_port.strip_prefix('[') {
        return v6.split_once(']').map(|(h, _)| h);
    }
    Some(host_port.split(':').next().unwrap_or(host_port))
}

/// This computer or the home network: localhost, loopback and private addresses, link-local ones,
/// .local / .lan / .home.arpa names and one-word names (a computer on the network, "desktop").
fn is_local_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    let Some(host) = host_of(&lower) else { return false };
    if host.is_empty() {
        return false;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            std::net::IpAddr::V6(v6) => {
                let first = v6.segments()[0];
                v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
            }
        };
    }
    host == "localhost"
        || [".localhost", ".local", ".lan", ".home.arpa", ".internal"].iter().any(|s| host.ends_with(s))
        || !host.contains('.')
}

/// Whether the AI is set up to answer: on, with a key (or a server on this computer or the home
/// network, which may need none).
pub fn ready(s: &AiSettings) -> bool {
    endpoint(s).is_ok()
}

const TRANSLATOR: &str = "You translate song lyrics for Cadence, a personal music player. Answer with strict JSON only: no markdown, no commentary.";

/// One JSON answer for a lyrics translation (`crate::translate`): no web search, little thinking,
/// a translator's instructions instead of the DJ's.
pub fn translate_json(s: &AiSettings, prompt: &str) -> Result<Value, String> {
    let ep = endpoint(s)?;
    if s.service == Service::Claude {
        let http = client(120)?;
        let messages = [json!({ "role": "user", "content": prompt })];
        let (mut body, fallbacks) = claude_body(&ep.model, &messages, false);
        body["system"] = json!(TRANSLATOR);
        if body.get("output_config").is_some() {
            body["output_config"] = json!({ "effort": "low" });
        }
        return claude_answer(&claude_send(&http, &ep, &body, fallbacks)?);
    }
    let http = client(120)?;
    let mut body = json!({
        "model": ep.model,
        "temperature": 0.3,
        "messages": [
            { "role": "system", "content": TRANSLATOR },
            { "role": "user", "content": prompt },
        ],
        "response_format": { "type": "json_object" },
    });
    let send = |body: &Value| {
        let mut req = http.post(format!("{}/chat/completions", ep.url)).json(body);
        if let Some(k) = &ep.key {
            req = req.bearer_auth(k);
        }
        req.send()
    };
    let mut r = send(&body).map_err(|e| unreachable_message(s, &e))?;
    // A service without JSON mode: asked again without it.
    if r.status() == reqwest::StatusCode::BAD_REQUEST {
        body.as_object_mut().map(|o| o.remove("response_format"));
        r = send(&body).map_err(|e| unreachable_message(s, &e))?;
    }
    let status = r.status();
    let v: Value = crate::net::json(r).map_err(|e| format!("Unexpected answer from the AI service: {e}"))?;
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => "The AI service didn’t accept the API key. Check it in Settings.".into(),
            404 => format!("The AI service doesn’t know the model “{}”. Check it in Settings.", ep.model),
            429 => "The AI service is busy or your free limit is used up. Try again later.".into(),
            _ => format!("The AI service answered {status}"),
        });
    }
    let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
    extract_json(text).ok_or_else(|| "The AI answer wasn’t in the expected format. Try again.".into())
}

/// Claude models that take a JSON schema for the answer (`output_config.format`).
fn claude_structured(model: &str) -> bool {
    ["claude-fable", "claude-mythos", "claude-opus-5", "claude-opus-4-8", "claude-sonnet-5", "claude-haiku-5", "claude-haiku-4-5", "claude-opus-4-5", "claude-opus-4-1"]
        .iter()
        .any(|m| model.starts_with(m))
}

/// The shortest prefix (tokens) a Claude model caches; shorter ones are silently not cached.
fn claude_cache_minimum(model: &str) -> usize {
    if ["claude-opus-5", "claude-fable", "claude-mythos", "claude-sonnet-5-5", "claude-haiku-5"].iter().any(|m| model.starts_with(m)) {
        512
    } else if model.starts_with("claude-opus-4-7") {
        2048
    } else if ["claude-opus-4-6", "claude-opus-4-5", "claude-haiku-4-5"].iter().any(|m| model.starts_with(m)) {
        4096
    } else {
        1024
    }
}

/// A low guess of a text's tokens (about 4 characters a token for English; other scripts take
/// more tokens per character, so this errs on the short side).
pub fn rough_tokens(text: &str) -> usize {
    text.len() / 4
}

/// One JSON answer for "Ask the DJ" (`crate::dj`): no web search, little thinking, the answer held
/// to `schema` where the service takes one (Claude's structured outputs, OpenAI-style
/// `json_schema`; services that don't are asked again with plain JSON mode, then without).
/// `system` never changes and `context` changes only with the library: both go first, and on
/// Claude they're marked for the prompt cache when together they're long enough to be cached.
/// `name`: what the request is for (the schema's name, and the log line with the token counts).
pub fn dj_json(s: &AiSettings, name: &str, system: &str, context: &str, prompt: &str, schema: &Value) -> Result<Value, String> {
    let ep = endpoint(s)?;
    let http = client(120)?;
    if s.service == Service::Claude {
        let (mut body, fallbacks) = claude_body(&ep.model, &[json!({ "role": "user", "content": prompt })], false);
        let mut blocks = vec![json!({ "type": "text", "text": system })];
        if !context.is_empty() {
            blocks.push(json!({ "type": "text", "text": context }));
        }
        if rough_tokens(system) + rough_tokens(context) + rough_tokens(&schema.to_string()) >= claude_cache_minimum(&ep.model) + 64
            && let Some(last) = blocks.last_mut()
        {
            last["cache_control"] = json!({ "type": "ephemeral" });
        }
        body["system"] = json!(blocks);
        body["max_tokens"] = json!(8000);
        let mut config = serde_json::Map::new();
        if body.get("output_config").is_some() {
            config.insert("effort".into(), json!("low"));
        }
        if claude_structured(&ep.model) {
            config.insert("format".into(), json!({ "type": "json_schema", "schema": schema }));
        }
        if config.is_empty() {
            body.as_object_mut().map(|o| o.remove("output_config"));
        } else {
            body["output_config"] = Value::Object(config);
        }
        let v = match claude_send(&http, &ep, &body, fallbacks) {
            Err(e) if e.starts_with("Claude answered 400") && body["output_config"].get("format").is_some() => {
                log::info!("Claude refused the answer's schema ({e}); asking without it");
                body["output_config"].as_object_mut().map(|o| o.remove("format"));
                claude_send(&http, &ep, &body, fallbacks)?
            }
            other => other?,
        };
        let u = &v["usage"];
        log::info!(
            "DJ {name}: {} tokens in ({} from the cache, {} cached now), {} out",
            u["input_tokens"].as_u64().unwrap_or(0),
            u["cache_read_input_tokens"].as_u64().unwrap_or(0),
            u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
            u["output_tokens"].as_u64().unwrap_or(0)
        );
        return claude_answer(&v);
    }
    let system_text = if context.is_empty() { system.to_string() } else { format!("{system}\n\n{context}") };
    let mut body = json!({
        "model": ep.model,
        "temperature": 0.4,
        "messages": [
            { "role": "system", "content": system_text },
            { "role": "user", "content": prompt },
        ],
        "response_format": { "type": "json_schema", "json_schema": { "name": format!("cadence_{name}"), "strict": true, "schema": schema } },
    });
    let send = |body: &Value| {
        let mut req = http.post(format!("{}/chat/completions", ep.url)).json(body);
        if let Some(k) = &ep.key {
            req = req.bearer_auth(k);
        }
        req.send()
    };
    let mut r = send(&body).map_err(|e| unreachable_message(s, &e))?;
    // No JSON schemas: JSON mode; no JSON mode either: plain (the answer is still checked).
    for fallback in [Some(json!({ "type": "json_object" })), None] {
        if r.status() != reqwest::StatusCode::BAD_REQUEST && r.status() != reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            break;
        }
        match fallback {
            Some(f) => body["response_format"] = f,
            None => {
                body.as_object_mut().map(|o| o.remove("response_format"));
            }
        }
        r = send(&body).map_err(|e| unreachable_message(s, &e))?;
    }
    let status = r.status();
    let v: Value = crate::net::json(r).map_err(|e| format!("Unexpected answer from the AI service: {e}"))?;
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => "The AI service didn’t accept the API key. Check it in Settings.".into(),
            404 => format!("The AI service doesn’t know the model “{}”. Check it in Settings.", ep.model),
            429 => "The AI service is busy or your free limit is used up. Try again later.".into(),
            _ => format!("The AI service answered {status}"),
        });
    }
    let u = &v["usage"];
    log::info!(
        "DJ {name}: {} tokens in ({} cached), {} out",
        u["prompt_tokens"].as_u64().unwrap_or(0),
        u["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0),
        u["completion_tokens"].as_u64().unwrap_or(0)
    );
    let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
    extract_json(text).ok_or_else(|| "The AI answer wasn’t in the expected format. Try again.".into())
}

/// The date and time for the DJ's plan ("Sunday 15 November 2026, evening (20:00)").
pub fn date_and_time_words() -> String {
    let local = crate::history::now() as i64 + crate::stats::local_offset();
    let day = local.div_euclid(86_400);
    let (y, m, d) = crate::stats::civil(day);
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    // 1970-01-01 was a Thursday.
    let weekday = DAYS[(day + 3).rem_euclid(7) as usize];
    format!("{weekday} {d} {} {y}, {}", MONTHS[(m as usize).saturating_sub(1).min(11)], now_words())
}

/// Claude answers one request (Messages API). With `search`, Claude searches the web itself (its
/// server-side web search tool) before answering; a turn the server pauses after many searches
/// is resumed. Returns the JSON object in the final answer.
fn claude_json(s: &AiSettings, ep: &Endpoint, prompt: &str, search: bool) -> Result<Value, String> {
    let search = search && s.web_search;
    let http = client(if search { 300 } else { 180 })?;
    let mut messages = vec![json!({ "role": "user", "content": prompt })];
    for _ in 0..4 {
        let (body, fallbacks) = claude_body(&ep.model, &messages, search);
        let v = claude_send(&http, ep, &body, fallbacks)?;
        // Many searches: the server paused the turn; send it back and it carries on.
        if v["stop_reason"] == "pause_turn" {
            messages.truncate(1);
            messages.push(json!({ "role": "assistant", "content": v["content"].clone() }));
            continue;
        }
        return claude_answer(&v);
    }
    Err("Claude took too many searches to answer. Try again.".into())
}

/// The Messages API request: the model, the DJ's instructions, the conversation, how hard to think,
/// web search when wanted, and the server-side refusal fallback where the model has one (returned
/// too: it needs its beta header).
fn claude_body(model: &str, messages: &[Value], search: bool) -> (Value, bool) {
    // The dynamic-filtering web search needs a current model; older ones (and Haiku) take the basic one.
    let current = ["claude-opus-5", "claude-sonnet-5", "claude-fable", "claude-mythos", "claude-opus-4-6", "claude-opus-4-7", "claude-opus-4-8", "claude-sonnet-4-6"];
    let search_tool = if current.iter().any(|m| model.starts_with(m)) { "web_search_20260209" } else { "web_search_20250305" };
    // On a decline, another model answers in the same call.
    let fallbacks = ["claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5", "claude-fable-5-1"].contains(&model);
    let mut body = json!({
        "model": model,
        "max_tokens": 16000,
        "system": "You are the DJ inside Cadence, a personal music player. Answer with strict JSON only: no markdown, no commentary.",
        "messages": messages,
    });
    if ["claude-opus-5", "claude-sonnet-5", "claude-fable", "claude-haiku-5"].iter().any(|m| model.starts_with(m)) {
        // Thinking is adaptive on these; effort says how hard to think (picking songs is light work).
        body["output_config"] = json!({ "effort": "medium" });
    }
    if fallbacks {
        body["fallbacks"] = json!("default");
    }
    if search {
        body["tools"] = json!([{ "type": search_tool, "name": "web_search", "max_uses": 6 }]);
    }
    (body, fallbacks)
}

/// The JSON object in Claude's answer: in its last text block (earlier ones may be notes between
/// searches), or in all the text together.
fn claude_answer(v: &Value) -> Result<Value, String> {
    if v["stop_reason"] == "refusal" {
        return Err("Claude declined this request. Try asking another way.".into());
    }
    let texts: Vec<&str> = v["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    texts
        .iter()
        .rev()
        .find_map(|t| extract_json(t))
        .or_else(|| extract_json(&texts.concat()))
        .ok_or_else(|| {
            if v["stop_reason"] == "max_tokens" {
                "Claude's answer was cut off. Try again.".into()
            } else {
                "Claude's answer wasn't in the expected format. Try again.".into()
            }
        })
}

/// One Messages API call, retried twice when Claude is busy (429, 529, 5xx).
fn claude_send(http: &reqwest::blocking::Client, ep: &Endpoint, body: &Value, fallbacks: bool) -> Result<Value, String> {
    let mut wait = Duration::from_secs(2);
    for attempt in 0..3 {
        let mut req = http
            .post(&ep.url)
            .header("anthropic-version", "2026-10-21")
            .header("x-api-key", ep.key.as_deref().unwrap_or(""))
            .json(body);
        if fallbacks {
            req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
        }
        let r = match req.send() {
            Ok(r) => r,
            Err(e) if e.is_timeout() => return Err("Claude took too long to answer. Try again.".into()),
            Err(e) => return Err(format!("Can’t reach Claude: {e}")),
        };
        let status = r.status().as_u16();
        let v: Value = crate::net::json(r).map_err(|e| format!("Unexpected answer from Claude: {e}"))?;
        if (200..300).contains(&status) {
            return Ok(v);
        }
        let retry = matches!(status, 429 | 500..=599);
        if retry && attempt < 2 {
            std::thread::sleep(wait);
            wait *= 3;
            continue;
        }
        let msg = v["error"]["message"].as_str().unwrap_or("no details");
        return Err(match status {
            401 | 403 => "Anthropic didn’t accept the API key. Check it in Settings.".into(),
            404 => format!("Anthropic doesn’t know the model “{}”. Check it in Settings.", ep.model),
            429 => "Claude is busy or your usage limit is reached. Try again later.".into(),
            529 => "Claude is overloaded right now. Try again in a minute.".into(),
            _ => format!("Claude answered {status}: {msg}"),
        });
    }
    unreachable!()
}

fn client(timeout: u64) -> Result<reqwest::blocking::Client, String> {
    crate::net::builder()
        .timeout(Duration::from_secs(timeout))
        .user_agent(concat!("Cadence/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("This system can’t make secure connections: {e}"))
}

pub const SYSTEM: &str = "You are the DJ inside Cadence, a personal music player. You pick songs only from \
the list you are given and answer with strict JSON only: no markdown, no commentary.";

/// Sends one chat request and returns the JSON object in the answer. `search`: allow web search.
fn chat_json(s: &AiSettings, prompt: &str, search: bool) -> Result<Value, String> {
    let ep = endpoint(s)?;
    if s.service == Service::Claude {
        return claude_json(s, &ep, prompt, search);
    }
    // Web search makes answers much slower (several searches before the reply).
    let http = client(if search && s.web_search { 300 } else { 120 })?;
    let mut body = json!({
        "model": ep.model,
        "temperature": 0.7,
        "messages": [
            { "role": "system", "content": SYSTEM },
            { "role": "user", "content": prompt },
        ],
        "response_format": { "type": "json_object" },
    });
    if search && s.web_search {
        // Z.ai's built-in web search tool.
        body["tools"] = json!([{
            "type": "web_search",
            "web_search": { "enable": true, "search_engine": "search-prime", "search_result": true, "count": 6 }
        }]);
    }
    let send = |body: &Value| {
        let mut req = http.post(format!("{}/chat/completions", ep.url)).json(body);
        if let Some(k) = &ep.key {
            req = req.bearer_auth(k);
        }
        req.send()
    };
    let mut r = send(&body).map_err(|e| unreachable_message(s, &e))?;
    // Some services (or plans) don't support web search or JSON mode; ask again without them.
    if r.status() == reqwest::StatusCode::BAD_REQUEST {
        if let Some(o) = body.as_object_mut() {
            if o.remove("tools").is_some() {
                log::info!("AI service refused web search; asking without it");
            } else {
                o.remove("response_format");
            }
        }
        r = send(&body).map_err(|e| unreachable_message(s, &e))?;
        if r.status() == reqwest::StatusCode::BAD_REQUEST {
            body.as_object_mut().map(|o| o.remove("response_format"));
            r = send(&body).map_err(|e| unreachable_message(s, &e))?;
        }
    }
    let status = r.status();
    let v: Value = crate::net::json(r).map_err(|e| format!("Unexpected answer from the AI service: {e}"))?;
    if !status.is_success() {
        let msg = v["error"]["message"]
            .as_str()
            .or(v["error"].as_str())
            .unwrap_or("no details");
        return Err(match status.as_u16() {
            401 | 403 => "The AI service didn’t accept the API key. Check it in Settings.".into(),
            404 => format!("The AI service doesn’t know the model “{}”. Check it in Settings.", ep.model),
            429 => "The AI service is busy or your free limit is used up. Try again later.".into(),
            _ => format!("The AI service answered {status}: {msg}"),
        });
    }
    let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
    extract_json(text).ok_or_else(|| "The AI answer wasn’t in the expected format. Try again.".into())
}

fn unreachable_message(_s: &AiSettings, e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "The AI took too long to answer. Try again.".into()
    } else {
        format!("Can’t reach the AI service: {e}")
    }
}

/// Finds the JSON object in a model's answer, skipping any "thinking" and code fences.
fn extract_json(text: &str) -> Option<Value> {
    let text = match text.rfind("</think>") {
        Some(i) => &text[i + 8..],
        None => text,
    };
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(text.get(start..=end)?).ok()
}

/// One song as the model sees it.
#[derive(Clone)]
pub struct Candidate {
    pub track: Track,
    /// A few words about its sound ("fast, bright, minor"), if analysed.
    pub sound: Option<String>,
}

/// A summary of the listener's habits, in plain words.
pub fn taste<'a>(track: impl Fn(&str) -> Option<&'a Track>, h: &HistoryStats) -> String {
    let mut artists: HashMap<&'a str, (u32, u32)> = HashMap::new();
    for (id, n) in &h.plays {
        if let Some(t) = track(id) {
            artists.entry(t.artist.as_str()).or_default().0 += n;
        }
    }
    for (id, n) in &h.skips {
        if let Some(t) = track(id) {
            artists.entry(t.artist.as_str()).or_default().1 += n;
        }
    }
    let mut liked: Vec<_> = artists.iter().filter(|(_, (p, _))| *p > 0).collect();
    liked.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    let skipped: Vec<_> = artists
        .iter()
        .filter(|(_, (p, s))| *s >= 3 && s > p)
        .map(|(a, _)| *a)
        .take(5)
        .collect();
    let mut out = String::new();
    if liked.is_empty() {
        out.push_str("No listening history yet.\n");
    } else {
        let names: Vec<_> = liked.iter().take(8).map(|(a, (p, _))| format!("{a} ({p} plays)")).collect();
        out.push_str(&format!("Most played artists: {}.\n", names.join(", ")));
    }
    if !skipped.is_empty() {
        out.push_str(&format!("Often skipped: {}.\n", skipped.join(", ")));
    }
    out
}

/// Chooses up to `SHORTLIST` songs to show the model: songs matching the request's words first,
/// then a rotating handful of favourites, then the rest spread across artists (so one artist with many albums can't fill the list).
/// `seed` picks which artists and songs come first, so each request sees a different slice.
pub fn shortlist(all: Vec<Candidate>, request: &str, h: &HistoryStats, seed: u64) -> Vec<Candidate> {
    if all.len() <= SHORTLIST {
        return all;
    }
    let words: Vec<String> = request
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(String::from)
        .collect();
    let score = |c: &Candidate| {
        let t = &c.track;
        let hay = format!("{} {} {} {} {}", t.title, t.artist, t.album, t.genre, c.sound.as_deref().unwrap_or(""))
            .to_lowercase();
        let hits = words.iter().filter(|w| hay.contains(w.as_str())).count() as f64;
        let plays = *h.plays.get(&t.id).unwrap_or(&0) as f64;
        let skips = *h.skips.get(&t.id).unwrap_or(&0) as f64;
        hits * 10.0 + (plays - 0.5 * skips).clamp(-3.0, 10.0) * 0.3
    };
    let mut scored: Vec<(f64, Candidate)> = all.into_iter().map(|c| (score(&c), c)).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    // Songs that match the request's words first.
    let matched = scored.iter().take(SHORTLIST / 2).take_while(|(s, _)| *s >= 10.0).count();
    let mut out: Vec<Candidate> = scored.drain(..matched).map(|(_, c)| c).collect();
    // Then a different handful of favourites each time, not always the same most played.
    let liked = scored.iter().take(SHORTLIST / 2).take_while(|(s, _)| *s > 0.0).count();
    let mut favourites: Vec<_> = scored.drain(..liked).collect();
    rng.shuffle(&mut favourites);
    let rest_of_liked = favourites.split_off(favourites.len().min(SHORTLIST / 6));
    out.extend(favourites.into_iter().map(|(_, c)| c));
    let rest = rest_of_liked.into_iter().chain(scored).enumerate();
    // Round-robin over artists, in a shuffled order, each artist's songs shuffled.
    let mut by_artist: Vec<(String, Vec<Candidate>)> = Vec::new();
    for (_, (_, c)) in rest {
        let key = c.track.artist.to_lowercase();
        match by_artist.iter_mut().find(|(a, _)| *a == key) {
            Some((_, v)) => v.push(c),
            None => by_artist.push((key, vec![c])),
        }
    }
    rng.shuffle(&mut by_artist);
    for (_, v) in by_artist.iter_mut() {
        rng.shuffle(v);
    }
    let mut queues: Vec<std::collections::VecDeque<Candidate>> = by_artist.into_iter().map(|(_, v)| v.into()).collect();
    while out.len() < SHORTLIST {
        let mut any = false;
        for v in queues.iter_mut() {
            if out.len() < SHORTLIST
                && let Some(c) = v.pop_front() {
                    out.push(c);
                    any = true;
                }
        }
        if !any {
            break;
        }
    }
    out
}

/// A small xorshift generator: enough to vary the shortlist, no extra crate.
pub(crate) struct Rng(pub u64);

impl Rng {
    /// A number in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

/// The song list as the model sees it (`id|title|artist|album|genre|year|plays|sound`), numbered
/// from `start + 1` (several lists sharing one set of ids).
fn catalog_from(c: &[Candidate], h: &HistoryStats, start: usize) -> String {
    let clean = |s: &str| s.replace(['|', '\n'], " ");
    c.iter()
        .enumerate()
        .map(|(i, c)| {
            let t = &c.track;
            format!(
                "{}|{}|{}|{}|{}|{}|{}|{}",
                start + i + 1,
                clean(&t.title),
                clean(&t.artist),
                clean(&t.album),
                clean(&t.genre),
                t.year.map(|y| y.to_string()).unwrap_or_default(),
                h.plays.get(&t.id).unwrap_or(&0),
                c.sound.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn mixes_prompt(taste: &str, pools: &[Pool], h: &HistoryStats, avoid: &[AiMix]) -> String {
    let before = if avoid.is_empty() {
        String::new()
    } else {
        let titles: Vec<_> = avoid.iter().map(|m| format!("\"{}\"", m.title)).collect();
        format!(
            "The listener already had these mixes today and wants new ones: {}. Don't reuse their titles or ideas.\n",
            titles.join(", ")
        )
    };
    let mut list = String::new();
    let mut start = 0;
    for (i, p) in pools.iter().enumerate() {
        list.push_str(&format!("POOL {}: {}\n{}\n\n", i + 1, p.brief(), catalog_from(&p.songs, h, start)));
        start += p.songs.len();
    }
    format!(
        "Now: {now}.\nLISTENER:\n{taste}\nSongs are listed as id|title|artist|album|genre|year|plays|sound.\n\n{list}\
Make one mix from each POOL, in the same order, using only that pool's songs. \
Each mix: 8–16 songs that belong together, ordered so they flow; leave out songs that don't fit the rest. \
At most 3 songs by one artist. \
title: at most 4 words, about the mood or the moment (not the pool's description); no two titles share a word. \
reason: one short sentence saying what the mix is, naming two or three of its artists.\n\
{before}\
Return JSON only: {{\"mixes\":[{{\"pool\":1,\"title\":\"\",\"reason\":\"\",\"ids\":[1,2,3]}}]}}",
        now = now_words(),
    )
}

fn now_words() -> String {
    // Local time of day matters more than the date ("late night", "morning").
    let now = local_hour();
    let part = match now {
        5..=11 => "morning",
        12..=16 => "afternoon",
        17..=21 => "evening",
        _ => "late night",
    };
    format!("{part} ({now}:00)")
}

/// The local hour, from Windows' local time.
pub fn local_hour() -> u32 {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct SystemTime {
            year: u16,
            month: u16,
            dow: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            ms: u16,
        }
        unsafe extern "system" {
            fn GetLocalTime(t: *mut SystemTime);
        }
        let mut t = SystemTime::default();
        unsafe { GetLocalTime(&mut t) };
        t.hour as u32
    }
    #[cfg(not(windows))]
    {
        ((crate::history::now() / 3600) % 24) as u32
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiMix {
    pub title: String,
    pub reason: String,
    pub track_ids: Vec<String>,
    /// Which part of the library a daily mix came from ("Rediscover"); empty for "Ask the DJ".
    #[serde(default)]
    pub lane: String,
    /// Songs not in the library the DJ thinks fit.
    #[serde(default)]
    pub discover: Vec<Discovery>,
    /// "Ask the DJ": what was understood ("Indonesian · calm · not played in 3 months · 25 songs").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub understood: String,
    /// "Ask the DJ": a constraint relaxed or not met, in one sentence ("" when none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notice: String,
    /// "Ask the DJ": why (and that) it was picked without the AI ("" when the AI did its part).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub without_ai: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Discovery {
    pub artist: String,
    pub title: String,
    pub why: String,
    /// Album it's on, from the catalogue.
    pub album: String,
    /// Album cover, or the artist's picture when `image_is_artist` (empty if unknown).
    pub image: String,
    pub image_is_artist: bool,
    /// Already looked up in the catalogue.
    pub checked: bool,
    /// How close it is to what was asked for and to the listener's taste, 0–100 (Discover).
    pub similarity: u8,
    /// How well listeners and critics rate the song (charts, streams, reviews), 0–100 (Discover).
    pub rating: u8,
    /// The song or artist in the listener's library it's most like (Discover).
    pub like: String,
    /// The song's names in the catalogue's other store, for recognising it in the library.
    pub other_names: OtherNames,
}

/// What a song is called in the catalogue's other store: Japanese releases are "米津玄師 -
/// KICK BACK" in Japan and "Kenshi Yonezu - KICK BACK" elsewhere, and a library may use either.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OtherNames {
    /// Looked up (even if the names turned out to be the same).
    pub checked: bool,
    pub artists: Vec<String>,
    pub titles: Vec<String>,
    pub albums: Vec<String>,
}

/// How often something was played: an artist, a genre or a song ("Artist - Title").
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Count {
    pub name: String,
    pub plays: u32,
}

/// What the listener plays: the last 30 days ("this month") and all time.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    pub month_artists: Vec<Count>,
    pub month_genres: Vec<Count>,
    pub month_songs: Vec<Count>,
    pub artists: Vec<Count>,
    pub genres: Vec<Count>,
    /// Favourite songs ("Artist - Title"): hearted or rated 4 stars or more, most played first.
    pub favorites: Vec<String>,
}

/// The last Discover search and what it found (kept in ai-recommend.json).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecommendCache {
    /// "month", "artist", "genre" or "search".
    pub basis: String,
    pub value: String,
    pub at: u64,
    pub picks: Vec<Discovery>,
}

/// A song Discover suggested before comes back only when it's rated at least this well (and the
/// listener still doesn't have it).
pub const REPEAT_AT: u8 = 85;

impl Discovery {
    /// Discover's order: mostly how close it is to the listener's songs, then how well it's rated.
    pub fn score(&self) -> u32 {
        self.similarity as u32 * 8 + self.rating as u32 * 2
    }
}

/// A 0–100 number from the model ("91", 91, "91%"; out of range clamped, missing 0).
fn percent(v: &Value) -> u8 {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim_end_matches('%').trim().parse().ok()))
        .unwrap_or(0.0)
        .clamp(0.0, 100.0) as u8
}

fn counts(c: &[Count], n: usize) -> String {
    if c.is_empty() {
        return "none yet".into();
    }
    c.iter().take(n).map(|c| format!("{} ({})", c.name, c.plays)).collect::<Vec<_>>().join(", ")
}

/// Discover: new songs (not in the library) for what's asked — the listener's last 30 days, an
/// artist, a genre, or anything said in words — each with how similar it is and to what.
pub fn recommend(s: &AiSettings, basis: &str, value: &str, taste: &str, p: &Profile, owned_artists: &[String], before: &[String]) -> Result<Vec<Discovery>, String> {
    let ask = match basis {
        "artist" => format!(
            "Songs for someone who loves {value}: newer songs by {value} that aren't in their library, and songs by \
artists who sound like {value}. similarity: how close each is to {value}'s sound."
        ),
        "genre" => format!(
            "{value} songs this listener would love, by artists they already like and by artists new to them. \
similarity: how well it fits {value} and their taste."
        ),
        "search" => format!("What they asked for: \"{value}\". similarity: how well it matches that and their taste."),
        _ => "Songs for what they're into this month (their last 30 days, below): like those artists, genres and \
songs. similarity: how close it is to this month's listening."
            .to_string(),
    };
    let prompt = format!(
        "Now: {now}.\nLISTENER (all time):\n{taste}\n\
THIS MONTH (last 30 days): top artists: {ma}. Top genres: {mg}. Top songs: {ms}.\n\
Their favourite songs: {fav}.\n\
Artists in their library, most played first ({owned_n} in all): {artists}\n{before}\nREQUEST: {ask}\n\n\
Suggest 15 real songs NOT in their library (not even on an album they have): they likely own the best-known songs \
of the artists listed first, so prefer songs they wouldn't have. Choose by how close each is to their top and \
favourite songs, whatever the year; at most 3 may be recent releases (this year and last), found on the web. Only \
real, existing songs, credited to the right artist. similarity: a whole number from 0 to 100. like: the one song \
(\"Artist - Title\") or artist from their listening above that it is most like. rating: how well listeners and \
critics rate the song, 0–100 (charts, streams, reviews). why: at most 12 words.\n\
Return JSON only: {{\"picks\":[{{\"artist\":\"\",\"title\":\"\",\"similarity\":0,\"rating\":0,\"like\":\"\",\"why\":\"\"}}]}}",
        now = now_words(),
        ma = counts(&p.month_artists, 15),
        mg = counts(&p.month_genres, 6),
        ms = counts(&p.month_songs, 25),
        fav = if p.favorites.is_empty() { "none marked".to_string() } else { p.favorites.join("; ") },
        owned_n = owned_artists.len(),
        artists = owned_artists.iter().take(crate::mixes::discover::OWNED_ARTISTS).map(String::as_str).collect::<Vec<_>>().join(", "),
        before = if before.is_empty() {
            String::new()
        } else {
            format!(
                "Suggested to them before: {}. Suggest one of these again only if it's very well rated (rating {REPEAT_AT} or more).\n",
                before.join("; ")
            )
        },
    );
    let v = chat_json(s, &prompt, true)?;
    let text = |v: &Value, k: &str| v[k].as_str().unwrap_or("").trim().to_string();
    let mut picks: Vec<Discovery> = v["picks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| Discovery {
            artist: text(d, "artist"),
            title: text(d, "title"),
            why: text(d, "why"),
            like: text(d, "like"),
            similarity: percent(&d["similarity"]),
            rating: percent(&d["rating"]),
            ..Default::default()
        })
        .filter(|d| !d.artist.is_empty() && !d.title.is_empty())
        .take(20)
        .collect();
    if picks.is_empty() {
        return Err("The DJ couldn’t find songs for that. Try again or ask another way.".into());
    }
    picks.sort_by(|a, b| b.score().cmp(&a.score()));
    Ok(distinct_picks(picks))
}

/// Each song once (the window lists picks by artist and title): the first of the same artist and
/// title, compared by letters and digits ("Lemon" and "LEMON!" are one song).
pub fn distinct_picks(picks: Vec<Discovery>) -> Vec<Discovery> {
    let mut seen = HashSet::new();
    picks
        .into_iter()
        .filter(|d| seen.insert((crate::state::match_key(&d.artist), crate::state::match_key(&d.title))))
        .collect()
}

/// Each mix title once (the window lists mixes by title): a repeated one gets " 2", " 3"…
pub fn distinct_titles(mixes: &mut [AiMix]) {
    let mut seen: HashSet<String> = HashSet::new();
    for m in mixes.iter_mut() {
        let base = m.title.trim().to_string();
        let mut title = base.clone();
        let mut n = 2;
        while !seen.insert(title.to_lowercase()) {
            title = format!("{base} {n}");
            n += 1;
        }
        m.title = title;
    }
}

fn to_mix(v: &Value, c: &[Candidate]) -> AiMix {
    let mut seen = HashSet::new();
    let track_ids = v["ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.trim().parse().ok())))
        .filter_map(|n| c.get((n as usize).checked_sub(1)?))
        .map(|c| c.track.id.clone())
        .filter(|id| seen.insert(id.clone()))
        .collect();
    let text = |v: &Value, k: &str| v[k].as_str().unwrap_or("").trim().to_string();
    let discover = v["discover"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| Discovery {
            artist: text(d, "artist"),
            title: text(d, "title"),
            why: text(d, "why"),
            ..Default::default()
        })
        .filter(|d| !d.artist.is_empty() && !d.title.is_empty())
        .take(6)
        .collect();
    AiMix {
        title: text(v, "title"),
        reason: text(v, "reason"),
        track_ids,
        lane: String::new(),
        discover: distinct_picks(discover),
        ..Default::default()
    }
}

/// Today's mixes, one per pool. The model's picks are checked against its own pool (no song
/// from elsewhere, at most 3 per artist); a pool it skipped or left too short is filled from the
/// pool itself, so every lane still gets a mix.
pub fn daily_mixes(
    s: &AiSettings,
    taste: &str,
    pools: &[Pool],
    h: &HistoryStats,
    avoid: &[AiMix],
) -> Result<Vec<AiMix>, String> {
    if pools.is_empty() {
        return Err("The library is too small for mixes yet.".into());
    }
    let v = chat_json(s, &mixes_prompt(taste, pools, h, avoid), false)?;
    let answers: Vec<&Value> = v["mixes"].as_array().into_iter().flatten().collect();
    let mixes = fit_to_pools(pools, &answers);
    if mixes.is_empty() {
        return Err("The DJ couldn’t make mixes this time. Try again.".into());
    }
    log_variety(&mixes, pools);
    Ok(mixes)
}

fn fit_to_pools(pools: &[Pool], answers: &[&Value]) -> Vec<AiMix> {
    let all: Vec<Candidate> = pools.iter().flat_map(|p| p.songs.iter().cloned()).collect();
    let mut mixes = Vec::new();
    for (i, p) in pools.iter().enumerate() {
        let answer = answers
            .iter()
            .find(|a| a["pool"].as_u64() == Some(i as u64 + 1))
            .or_else(|| answers.get(i).filter(|a| a["pool"].is_null()));
        let mut m = answer.map(|a| to_mix(a, &all)).unwrap_or_else(|| to_mix(&Value::Null, &all));
        let artist: HashMap<&str, String> = p.songs.iter().map(|c| (c.track.id.as_str(), crate::lanes::artist_key(c))).collect();
        let mut per: HashMap<String, usize> = HashMap::new();
        m.track_ids.retain(|id| {
            artist.get(id.as_str()).is_some_and(|a| {
                let n = per.entry(a.clone()).or_default();
                *n += 1;
                *n <= 3
            })
        });
        if m.track_ids.len() < 6 {
            for c in &p.songs {
                if m.track_ids.len() >= 12 {
                    break;
                }
                let n = per.entry(crate::lanes::artist_key(c)).or_default();
                if *n < 3 && !m.track_ids.contains(&c.track.id) {
                    *n += 1;
                    m.track_ids.push(c.track.id.clone());
                }
            }
        }
        if m.title.is_empty() {
            m.title = p.lane.label().to_string();
        }
        if m.reason.is_empty() {
            let b = p.brief();
            m.reason = format!("{}{}.", b[..1].to_uppercase(), &b[1..]);
        }
        m.lane = p.lane.label().to_string();
        m.discover.clear();
        mixes.push(m);
    }
    distinct_mixes(mixes)
}

/// One log line per day's mixes, so changes to the lanes can be compared by numbers, not by eye.
fn log_variety(mixes: &[AiMix], pools: &[Pool]) {
    let artist: HashMap<&str, String> = pools
        .iter()
        .flat_map(|p| p.songs.iter())
        .map(|c| (c.track.id.as_str(), crate::lanes::artist_key(c)))
        .collect();
    let sets: Vec<HashSet<&String>> = mixes
        .iter()
        .map(|m| m.track_ids.iter().filter_map(|id| artist.get(id.as_str())).collect())
        .collect();
    let mut shared = 0;
    for i in 0..sets.len() {
        for j in i + 1..sets.len() {
            shared += sets[i].intersection(&sets[j]).count();
        }
    }
    let summary: Vec<String> = mixes
        .iter()
        .zip(&sets)
        .map(|(m, s)| format!("{} ({}: {} songs, {} artists)", m.title, m.lane, m.track_ids.len(), s.len()))
        .collect();
    log::info!("AI mixes: {}; artists shared between mixes: {shared}", summary.join(", "));
}

/// Keeps each song in the first mix that has it, then drops mixes left too short or untitled, and
/// gives each mix its own title.
fn distinct_mixes(mixes: Vec<AiMix>) -> Vec<AiMix> {
    let mut used = HashSet::new();
    let mut out: Vec<AiMix> = mixes
        .into_iter()
        .map(|mut m| {
            m.track_ids.retain(|id| used.insert(id.clone()));
            m
        })
        .filter(|m| m.track_ids.len() >= 3 && !m.title.trim().is_empty())
        .collect();
    distinct_titles(&mut out);
    out
}

/// A quick check that the key, server and model work.
pub fn test(s: &AiSettings) -> Result<(), String> {
    let v = chat_json(s, "Reply with this JSON only: {\"ok\":true}", false)?;
    if v["ok"].as_bool() == Some(true) {
        Ok(())
    } else {
        Err("The AI answered, but not as expected. Check the model name.".into())
    }
}

/// What the AI found for a song's lyrics.
pub struct FoundLyrics {
    /// Other ways to search for the song (other languages or spellings of the title and artist).
    pub queries: Vec<String>,
    /// Time-synced LRC if it found one.
    pub lrc: String,
    /// Plain lyrics, line by line.
    pub plain: String,
    pub instrumental: bool,
}

/// Asks the AI (with web search) to find a song's lyrics.
pub fn find_lyrics(s: &AiSettings, t: &Track) -> Result<FoundLyrics, String> {
    let prompt = format!(
        "Find the lyrics of this song. Search the web.\n\
Title: {title}\nArtist: {artist}\nAlbum: {album}\nLength: {len}\n\n\
Return JSON only: {{\"queries\":[\"\"],\"lrc\":\"\",\"lyrics\":\"\",\"instrumental\":false}}\n\
queries: up to 4 short searches (title and main artist) that a lyrics database would match, \
including the title in other languages or spellings used for this song.\n\
lrc: time-synced lyrics in LRC format ([mm:ss.xx] line) only if you found real timings, else \"\".\n\
lyrics: the complete original lyrics, one line per line, exactly as published; \"\" if you can't find them. \
Never invent or translate lyrics.\ninstrumental: true if the song has no vocals.",
        title = t.title,
        artist = t.artist,
        album = t.album,
        len = format!("{}:{:02}", (t.duration / 60.0) as u32, (t.duration % 60.0) as u32),
    );
    let v = chat_json(s, &prompt, true)?;
    let text = |k: &str| v[k].as_str().unwrap_or("").trim().to_string();
    Ok(FoundLyrics {
        queries: v["queries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|q| q.as_str().map(|q| q.trim().to_string()))
            .filter(|q| !q.is_empty())
            .take(4)
            .collect(),
        lrc: text("lrc"),
        plain: text("lyrics"),
        instrumental: v["instrumental"].as_bool() == Some(true),
    })
}

/// Who sings a song: its singers, and per line who sings it when published lyrics say so.
pub struct FoundSingers {
    pub singers: Vec<crate::voice::AiSinger>,
    /// Per line (as given), indexes into `singers`; empty when no published parts were found.
    pub parts: Vec<Vec<u8>>,
}

/// Asks the AI (with web search) who sings this song and which lines each singer sings.
pub fn find_singers(s: &AiSettings, t: &Track, lines: &[String]) -> Result<FoundSingers, String> {
    let numbered: String = lines.iter().enumerate().map(|(i, l)| format!("{i}: {l}\n")).collect();
    let prompt = format!(
        "Who sings this song, and who sings each line? Search the web: credits, the official lyrics, \
lyrics sites that mark each singer's part (color-coded or with names), fan wikis.\n\
Title: {title}\nArtist: {artist}\nAlbum: {album}\n\nThe lyrics, numbered:\n{numbered}\n\
Return JSON only: {{\"singers\":[{{\"name\":\"\",\"voice\":\"\",\"credit\":\"\"}}],\"parts\":[[0]],\"parts_source\":\"\"}}\n\
singers: the vocalists credited on THIS recording (not producers, composers or band members who play \
instruments), in the order they first sing. credit: the URL of the page that credits them as a vocalist on \
this song or its album (official site, liner notes, Discogs, VGMdb, the label). A singer you can't cite a \
credit for must be left out: never name a vocalist from memory or because they are in the band.\n\
voice: \"lower\" or \"higher\" when there are two credited singers and you know whose voice is lower (e.g. a \
man and a woman), else \"\".\n\
parts: one entry per numbered line, in order ({n} entries): the indexes in singers of who sings that line \
([0,1] when they sing it together, [] for an empty line). Only fill parts from a real source that marks the \
parts, and give its URL in parts_source; if you didn't find one, return \"parts\":[] and \"parts_source\":\"\". \
Never guess.",
        title = t.title,
        artist = t.artist,
        album = t.album,
        n = lines.len(),
    );
    let v = chat_json(s, &prompt, true)?;
    Ok(read_singers(&v, lines.len()))
}

/// A web address the AI cites (anything else is no source).
fn is_url(s: &str) -> bool {
    let s = s.trim();
    (s.starts_with("https://") || s.starts_with("http://")) && s.len() > 12 && !s.contains(char::is_whitespace)
}

/// The AI's answer about the singers, keeping only what it cites a source for: singers without a
/// credit are dropped (a model will otherwise name a band member who only plays guitar), and the
/// parts need a source and one entry per line.
fn read_singers(v: &Value, lines: usize) -> FoundSingers {
    let singers: Vec<crate::voice::AiSinger> = v["singers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| {
            let name = x["name"].as_str()?.trim().to_string();
            let credit = x["credit"].as_str().unwrap_or("").trim().to_string();
            if name.is_empty() || !is_url(&credit) {
                return None;
            }
            let voice = x["voice"].as_str().unwrap_or("").trim().to_lowercase();
            Some(crate::voice::AiSinger {
                name,
                voice: if voice == "lower" || voice == "higher" { voice } else { String::new() },
                credit,
            })
        })
        .take(12)
        .collect();
    // Whose voice is lower only means something with two credited singers.
    let singers = if singers.len() == 2 {
        singers
    } else {
        singers.into_iter().map(|s| crate::voice::AiSinger { voice: String::new(), ..s }).collect()
    };
    let n = singers.len();
    let parts: Vec<Vec<u8>> = v["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            p.as_array()
                .into_iter()
                .flatten()
                .filter_map(|i| i.as_u64())
                .filter(|&i| (i as usize) < n)
                .map(|i| i as u8)
                .collect()
        })
        .collect();
    let sourced = is_url(v["parts_source"].as_str().unwrap_or(""));
    FoundSingers {
        // Parts without a source, or that don't match the lines, are of no use.
        parts: if sourced && parts.len() == lines { parts } else { Vec::new() },
        singers,
    }
}

/// Songs the listener doesn't have yet, picked from their listening (taste, most played songs)
/// with web search for recent releases.


/// Today's mixes, kept so they're only made once a day.
#[derive(Default, Serialize, Deserialize)]
pub struct DailyCache {
    pub day: u64,
    pub mixes: Vec<AiMix>,
    /// Songs in the mixes of recent days (day, ids), so tomorrow's mixes don't repeat them.
    #[serde(default)]
    pub recent: Vec<(u64, Vec<String>)>,
    /// Why today's attempt failed; it isn't retried automatically until tomorrow.
    #[serde(default)]
    pub error: String,
}

pub fn today() -> u64 {
    // Days in local time would be nicer; UTC days change at 07:00 in Jakarta, close enough to morning.
    crate::history::now() / 86_400
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_pick_and_each_mix_title_once() {
        let pick = |a: &str, t: &str, why: &str| Discovery { artist: a.into(), title: t.into(), why: why.into(), ..Default::default() };
        let picks = distinct_picks(vec![pick("Kenshi Yonezu", "Lemon", "first"), pick("kenshi yonezu", "LEMON!", "again"), pick("Kenshi Yonezu", "Flamingo", "")]);
        assert_eq!(picks.iter().map(|d| d.why.as_str()).collect::<Vec<_>>(), ["first", ""]);
        let mix = |title: &str, ids: &[&str]| AiMix {
            title: title.into(),
            reason: String::new(),
            track_ids: ids.iter().map(|s| s.to_string()).collect(),
            lane: String::new(),
            discover: Vec::new(),
            ..Default::default()
        };
        let mixes = distinct_mixes(vec![mix("Night Drive", &["a", "b", "c"]), mix("night drive", &["d", "e", "f"]), mix("Night Drive", &["g", "h", "i"])]);
        assert_eq!(mixes.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["Night Drive", "night drive 2", "Night Drive 3"]);
    }

    #[test]
    fn the_key_goes_only_to_safe_addresses() {
        for ok in ["https://api.z.ai/api/coding/paas/v4", "http://localhost:11434/v1", "http://127.0.0.1:1234/v1", "http://192.168.1.4:11434/v1", "http://10.0.0.2/v1", "http://[::1]:11434/v1", "http://desktop:11434/v1", "http://nas.local/v1"] {
            assert!(check_url(ok).is_ok(), "{ok}");
        }
        for bad in ["http://api.example.com/v1", "http://8.8.8.8/v1", "ftp://x", "api.example.com", "https://", "http://user@evil.com:80/v1"] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
        assert!(is_local_url("http://localhost:11434/v1") && !is_local_url("https://api.z.ai/v4"));
        assert!(!is_local_url("http://172.32.0.1/v1") && is_local_url("http://172.16.5.1/v1"));
    }

    #[test]
    fn shortlist_spreads_and_varies() {
        // One artist with 300 songs and 20 artists with 5 each: the big one can't take over.
        let song = |artist: &str, n: usize| Candidate {
            track: Track { id: format!("{artist}-{n}"), artist: artist.into(), ..Default::default() },
            sound: None,
        };
        let all = || {
            let mut v: Vec<_> = (0..300).map(|n| song("Aimer", n)).collect();
            v.extend((0..20).flat_map(|a| (0..5).map(move |n| song(&format!("A{a}"), n))));
            v
        };
        let h = HistoryStats::default();
        let a = shortlist(all(), "", &h, 1);
        assert_eq!(a.len(), SHORTLIST);
        assert!(a.iter().filter(|c| c.track.artist == "Aimer").count() < 60);
        let b = shortlist(all(), "", &h, 2);
        let ids = |v: &[Candidate]| v.iter().map(|c| c.track.id.clone()).collect::<Vec<_>>();
        assert_ne!(ids(&a), ids(&b));
        // A song lands in one mix only; a mix left with fewer than 3 songs is dropped.
        let mix = |t: &str, ids: &[&str]| AiMix {
            title: t.into(),
            reason: String::new(),
            track_ids: ids.iter().map(|s| s.to_string()).collect(),
            lane: String::new(),
            discover: Vec::new(),
            ..Default::default()
        };
        let out = distinct_mixes(vec![mix("A", &["1", "2", "3"]), mix("B", &["1", "2", "4"]), mix("C", &["3", "5", "6", "7"])]);
        assert_eq!(out.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["A", "C"]);
        assert_eq!(out[1].track_ids, ["5", "6", "7"]);
    }

    #[test]
    fn mixes_stay_in_their_pools() {
        use crate::lanes::{Lane, Pool};
        let song = |artist: &str, n: usize| Candidate {
            track: Track { id: format!("{artist}-{n}"), artist: artist.into(), ..Default::default() },
            sound: None,
        };
        let pool = |lane, artists: &[&str]| Pool {
            lane,
            note: String::new(),
            songs: artists.iter().flat_map(|a| (0..4).map(|n| song(a, n))).collect(),
        };
        let pools = [pool(Lane::RightNow, &["A", "B", "C"]), pool(Lane::DeepCuts, &["D", "E", "F"])];
        // Pool 1: five songs by A (ids 1–4 are A's four songs) plus one from pool 2 (id 13).
        // Pool 2: missing from the answer.
        let answer = json!({ "pool": 1, "title": "Morning", "reason": "r", "ids": [1, 2, 3, 4, 5, 6, 7, 13] });
        let mixes = fit_to_pools(&pools, &[&answer]);
        assert_eq!(mixes.len(), 2);
        assert_eq!(mixes[0].lane, "Right now");
        assert_eq!(mixes[0].track_ids.iter().filter(|id| id.starts_with("A-")).count(), 3);
        assert!(mixes[0].track_ids.iter().all(|id| !id.starts_with("D-")));
        // The missing pool still gets a mix, from its own songs, named after its lane.
        assert_eq!(mixes[1].title, "Deep cuts");
        assert!(mixes[1].track_ids.len() >= 8);
        assert!(mixes[1].track_ids.iter().all(|id| ["D-", "E-", "F-"].iter().any(|p| id.starts_with(p))));
    }

    #[test]
    fn singers_need_a_credit() {
        // A band member named from memory (no credit) is left out, and with one credited singer
        // left, "lower"/"higher" means nothing.
        let v = json!({
            "singers": [
                { "name": "Cassie Wei", "voice": "higher", "credit": "https://vgmdb.net/album/62456" },
                { "name": "Yamato Kasai", "voice": "lower", "credit": "" },
            ],
            "parts": [[0], [1]],
            "parts_source": "",
        });
        let f = read_singers(&v, 2);
        assert_eq!(f.singers.len(), 1);
        assert_eq!(f.singers[0].name, "Cassie Wei");
        assert_eq!(f.singers[0].voice, "");
        // Parts without a source are dropped too.
        assert!(f.parts.is_empty());
        // Two credited singers with sourced parts: all kept.
        let v = json!({
            "singers": [
                { "name": "A", "voice": "lower", "credit": "https://example.com/credits" },
                { "name": "B", "voice": "higher", "credit": "https://example.com/credits" },
            ],
            "parts": [[0], [0, 1]],
            "parts_source": "https://example.com/lyrics",
        });
        let f = read_singers(&v, 2);
        assert_eq!(f.singers.iter().map(|s| s.voice.as_str()).collect::<Vec<_>>(), ["lower", "higher"]);
        assert_eq!(f.parts, vec![vec![0], vec![0, 1]]);
        // Parts for the wrong number of lines: dropped.
        assert!(read_singers(&v, 3).parts.is_empty());
    }

    #[test]
    fn claude_request() {
        let msgs = [json!({ "role": "user", "content": "hi" })];
        let (body, fallbacks) = claude_body("claude-opus-5-5", &msgs, true);
        assert!(fallbacks);
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["output_config"]["effort"], "medium");
        assert_eq!(body["tools"][0]["type"], "web_search_20260209");
        assert_eq!(body["tools"][0]["name"], "web_search");
        assert!(body.get("thinking").is_none() && body.get("temperature").is_none());
        // An older model: the basic web search, no fallback or effort.
        let (body, fallbacks) = claude_body("claude-haiku-4-5", &msgs, true);
        assert!(!fallbacks && body.get("fallbacks").is_none() && body.get("output_config").is_none());
        assert_eq!(body["tools"][0]["type"], "web_search_20250305");
        let (body, _) = claude_body("claude-opus-5-5", &msgs, false);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn claude_answer_after_searching() {
        // The shape of an answer that searched the web: a note, the search, its results, then
        // the answer (cited, split into text blocks), and the JSON last.
        let v = json!({
            "stop_reason": "end_turn",
            "content": [
                { "type": "text", "text": "I'll look up recent releases {maybe}." },
                { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": { "query": "YOASOBI 2026" } },
                { "type": "web_search_tool_result", "tool_use_id": "srvtoolu_1", "content": [{ "type": "web_search_result", "url": "https://example.com", "title": "x" }] },
                { "type": "text", "text": "Found some.", "citations": [{ "type": "web_search_result_location", "url": "https://example.com" }] },
                { "type": "text", "text": "{\"picks\":[{\"artist\":\"YOASOBI\",\"title\":\"Idol\",\"similarity\":91}]}" },
            ],
        });
        let a = claude_answer(&v).unwrap();
        assert_eq!(a["picks"][0]["similarity"], 91);
        assert!(claude_answer(&json!({ "stop_reason": "refusal", "content": [] })).is_err());
        assert!(claude_answer(&json!({ "stop_reason": "max_tokens", "content": [{ "type": "text", "text": "{\"picks\":[" }] })).unwrap_err().contains("cut off"));
    }

    /// Live, costs a little: `ANTHROPIC_API_KEY=… cargo test --lib ai::tests::claude_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn claude_live() {
        let key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY");
        let ep = Endpoint { url: "https://api.anthropic.com/v1/messages".into(), model: CLAUDE_MODEL.into(), key: Some(key) };
        let s = AiSettings { provider: Provider::Cloud, ..Default::default() };
        let v = claude_json(&s, &ep, "Search the web for one song released this year by YOASOBI. Return JSON only: {\"artist\":\"\",\"title\":\"\"}", true).unwrap();
        println!("{v}");
        assert!(v["title"].as_str().is_some_and(|t| !t.is_empty()));
    }

    #[test]
    fn finds_json_after_thinking() {
        let v = extract_json("<think>maybe {a}</think>\n```json\n{\"ids\":[1,2]}\n```").unwrap();
        assert_eq!(v["ids"][1], 2);
        assert!(extract_json("no json here").is_none());
    }
}
