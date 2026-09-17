//! Kubuno STT module — self-hosted speech-to-text.
//!
//! One in-process engine (no execve): Whisper (whisper.cpp), compiled from
//! source and linked statically, so the module ships as a self-contained
//! `.kbpkg` on Linux/Windows/macOS with no external native library to install.
//! Whisper is multilingual — one model serves every language. Models are
//! downloaded/deleted on demand; audio never leaves the server.

mod catalog;
mod text;

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::{
    body::Bytes,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::{DefaultBodyLimit, Path as AxPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::mpsc as std_mpsc;
use tokio::sync::mpsc as tok_mpsc;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const HOST: &str = "127.0.0.1";
const PORT: u16 = 3122;

fn default_true() -> bool { true }
fn default_beam() -> u32 { 1 }

fn default_engine() -> String { "whisper".into() }

#[derive(Clone, Serialize, Deserialize)]
struct LangCfg {
    // Whisper is the only engine now. Kept (defaulting to "whisper") so older
    // config files — including ones that named "vosk" — still deserialize.
    #[serde(default = "default_engine")] engine: String,
    model:  String,   // model id
    // Per-language options (all default to a sensible value, so older config
    // files lacking them still deserialize).
    #[serde(default = "default_true")]  enabled: bool,            // offer this language
    #[serde(default)]                   initial_prompt: String,   // Whisper: vocabulary/spelling bias
    #[serde(default)]                   grammar: String,          // legacy (Vosk word list) — unused, kept for config back-compat
    #[serde(default)]                   normalize_numbers: bool,  // spoken numbers → digits
    #[serde(default = "default_true")]  punctuation: bool,        // keep punctuation (Whisper); strip when false
    #[serde(default)]                   translate: bool,          // Whisper: translate to English
    #[serde(default = "default_beam")]  beam_size: u32,           // Whisper: 1 = greedy (fast), >1 = beam search (accurate)
    #[serde(default)]                   auto_detect: bool,        // Whisper: auto-detect spoken language
}

impl LangCfg {
    fn whisper(model: String) -> Self {
        LangCfg {
            engine: "whisper".into(), model,
            enabled: true, initial_prompt: String::new(), grammar: String::new(),
            normalize_numbers: false, punctuation: true, translate: false,
            beam_size: 1, auto_detect: false,
        }
    }
}

type Config = HashMap<String, LangCfg>;

fn default_silence() -> u32 { 3000 }
fn default_threshold() -> f32 { 0.04 }

/// Global capture/output settings (not per-language).
#[derive(Clone, Serialize, Deserialize)]
struct GlobalSettings {
    #[serde(default = "default_silence")]   silence_ms: u32,        // auto-stop after this much silence
    #[serde(default = "default_threshold")] sound_threshold: f32,   // mic level considered "sound"
    #[serde(default)]                       profanity_filter: bool, // mask profanity in output
}

impl Default for GlobalSettings {
    fn default() -> Self {
        GlobalSettings { silence_ms: default_silence(), sound_threshold: default_threshold(), profanity_filter: false }
    }
}

#[derive(Clone, Serialize)]
struct DownloadStatus {
    state:    String,   // "downloading" | "done" | "error"
    received: u64,
    total:    u64,
    error:    Option<String>,
}

#[derive(Clone)]
struct AppState {
    models_dir: PathBuf,
    data_dir:   PathBuf,
    config:     Arc<Mutex<Config>>,
    enabled:    Arc<Mutex<bool>>,   // global on/off switch (admin-controlled)
    settings:   Arc<Mutex<GlobalSettings>>,
    whisper_cache: Arc<Mutex<HashMap<PathBuf, Arc<WhisperContext>>>>,
    downloads:  Arc<Mutex<HashMap<String, DownloadStatus>>>,
}

// ── Config persistence ───────────────────────────────────────────────────────
fn config_path(state: &AppState) -> PathBuf { state.data_dir.join("config.json") }

fn load_config(state: &AppState) -> Config {
    std::fs::read(config_path(state))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_config(state: &AppState, cfg: &Config) {
    let _ = std::fs::create_dir_all(&state.data_dir);
    if let Ok(b) = serde_json::to_vec_pretty(cfg) {
        let _ = std::fs::write(config_path(state), b);
    }
}

// ── Global enabled flag (defaults to true) ───────────────────────────────────
fn enabled_path(state: &AppState) -> PathBuf { state.data_dir.join("enabled.json") }

fn load_enabled(state: &AppState) -> bool {
    std::fs::read(enabled_path(state))
        .ok()
        .and_then(|b| serde_json::from_slice::<bool>(&b).ok())
        .unwrap_or(true)
}

fn save_enabled(state: &AppState, v: bool) {
    let _ = std::fs::create_dir_all(&state.data_dir);
    if let Ok(b) = serde_json::to_vec(&v) {
        let _ = std::fs::write(enabled_path(state), b);
    }
}

fn is_enabled(state: &AppState) -> bool { *state.enabled.lock().expect("enabled poisoned") }

/// Whether a specific language is offered (global switch AND per-language flag).
fn lang_enabled(state: &AppState, lang: &str) -> bool {
    if !is_enabled(state) { return false; }
    state.config.lock().expect("cfg poisoned").get(lang).map(|c| c.enabled).unwrap_or(true)
}

// ── Global settings persistence ──────────────────────────────────────────────
fn settings_path(state: &AppState) -> PathBuf { state.data_dir.join("settings.json") }

fn load_settings(state: &AppState) -> GlobalSettings {
    std::fs::read(settings_path(state))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_settings(state: &AppState, s: &GlobalSettings) {
    let _ = std::fs::create_dir_all(&state.data_dir);
    if let Ok(b) = serde_json::to_vec_pretty(s) {
        let _ = std::fs::write(settings_path(state), b);
    }
}

// ── Installed models ─────────────────────────────────────────────────────────
fn whisper_dir(state: &AppState) -> PathBuf { state.models_dir.join("whisper") }

/// Set of installed model ids, e.g. {"whisper": [..]}. Kept as a map keyed by
/// engine for API stability (the admin UI reads `installed.whisper`).
fn installed(state: &AppState) -> HashMap<String, Vec<String>> {
    let mut whisper = Vec::new();
    if let Ok(rd) = std::fs::read_dir(whisper_dir(state)) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(stem) = name.strip_suffix(".bin") { whisper.push(stem.to_string()); }
        }
    }
    HashMap::from([("whisper".into(), whisper)])
}

/// Resolve (LangCfg, model_path) for a language: the configured Whisper model if
/// present, else any installed Whisper model (multilingual — one serves every
/// language).
fn resolve_for_lang(state: &AppState, lang: &str) -> anyhow::Result<(LangCfg, PathBuf)> {
    let cfg = state.config.lock().expect("config poisoned").clone();
    if let Some(c) = cfg.get(lang) {
        let path = whisper_dir(state).join(format!("{}.bin", c.model));
        if path.exists() { return Ok((c.clone(), path)); }
    }
    // Fallback: any installed Whisper model.
    let inst = installed(state);
    if let Some(any) = inst.get("whisper").and_then(|v| v.first()) {
        return Ok((LangCfg::whisper(any.clone()), whisper_dir(state).join(format!("{any}.bin"))));
    }
    anyhow::bail!("no whisper model installed for language '{lang}'")
}

// ── Output post-processing (shared by batch + streaming) ──────────────────────
fn post_process(state: &AppState, cfg: &LangCfg, lang: &str, raw: &str) -> String {
    let settings = state.settings.lock().expect("settings poisoned").clone();
    let mut t = raw.to_string();
    if cfg.normalize_numbers { t = text::normalize_numbers(&t, lang); }
    if !cfg.punctuation { t = text::strip_punctuation(&t); }
    if settings.profanity_filter { t = text::filter_profanity(&t, lang); }
    t.trim().to_string()
}

// ── Engine: Whisper ──────────────────────────────────────────────────────────
fn whisper_ctx(state: &AppState, path: &Path) -> anyhow::Result<Arc<WhisperContext>> {
    let mut cache = state.whisper_cache.lock().expect("whisper cache poisoned");
    if let Some(c) = cache.get(path) { return Ok(c.clone()); }
    let ctx = WhisperContext::new_with_params(
        path.to_string_lossy().as_ref(),
        WhisperContextParameters::default(),
    )?;
    let ctx = Arc::new(ctx);
    cache.insert(path.to_path_buf(), ctx.clone());
    Ok(ctx)
}

/// i16 PCM @ src_rate → f32 mono @ 16 kHz (linear resample) for Whisper.
fn to_f32_16k(samples: &[i16], src_rate: u32) -> Vec<f32> {
    if src_rate == 16000 {
        return samples.iter().map(|&s| s as f32 / 32768.0).collect();
    }
    let ratio = 16000f64 / src_rate as f64;
    let out_len = (samples.len() as f64 * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 / ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let a = *samples.get(idx).unwrap_or(&0) as f32 / 32768.0;
        let b = *samples.get(idx + 1).unwrap_or(&0) as f32 / 32768.0;
        out.push(a + (b - a) * frac);
    }
    out
}

fn whisper_transcribe(ctx: &WhisperContext, audio16k: &[f32], lang: &str, cfg: &LangCfg) -> anyhow::Result<String> {
    let mut st = ctx.create_state()?;
    let strategy = if cfg.beam_size > 1 {
        SamplingStrategy::BeamSearch { beam_size: cfg.beam_size as i32, patience: -1.0 }
    } else {
        SamplingStrategy::Greedy { best_of: 1 }
    };
    let mut p = FullParams::new(strategy);
    // "auto" lets whisper detect the language; otherwise force the configured one.
    p.set_language(Some(if cfg.auto_detect { "auto" } else { lang }));
    p.set_translate(cfg.translate);
    if !cfg.initial_prompt.is_empty() {
        p.set_initial_prompt(&cfg.initial_prompt);
    }
    p.set_print_special(false);
    p.set_print_progress(false);
    p.set_print_realtime(false);
    p.set_print_timestamps(false);
    st.full(p, audio16k)?;
    let n = st.full_n_segments();
    let mut text = String::new();
    for i in 0..n {
        if let Some(seg) = st.get_segment(i) {
            text.push_str(&seg.to_str_lossy()?);
        }
    }
    Ok(text.trim().to_string())
}

// ── Batch transcription (WAV) ────────────────────────────────────────────────
fn transcribe_wav(state: &AppState, lang: &str, wav: &[u8]) -> anyhow::Result<String> {
    let reader = hound::WavReader::new(Cursor::new(wav))?;
    let rate = reader.spec().sample_rate;
    let samples: Vec<i16> = reader.into_samples::<i16>().collect::<Result<_, _>>()?;
    let (cfg, path) = resolve_for_lang(state, lang)?;
    let ctx = whisper_ctx(state, &path)?;
    let raw = whisper_transcribe(&ctx, &to_f32_16k(&samples, rate), lang, &cfg)?;
    Ok(post_process(state, &cfg, lang, &raw))
}

async fn health() -> impl IntoResponse { Json(json!({ "status": "ok" })) }

/// Public status — lets the shell decide whether to show the mic button and how
/// to drive the silence auto-stop. `?lang=` reports per-language availability.
async fn status(State(state): State<AppState>, Query(q): Query<LangQuery>) -> impl IntoResponse {
    let enabled = match q.lang {
        Some(ref l) => lang_enabled(&state, l),
        None => is_enabled(&state),
    };
    let s = state.settings.lock().expect("settings poisoned").clone();
    Json(json!({
        "enabled":         enabled,
        "silence_ms":      s.silence_ms,
        "sound_threshold": s.sound_threshold,
    }))
}

#[derive(Deserialize)]
struct LangQuery { lang: Option<String> }

async fn transcribe_handler(
    State(state): State<AppState>,
    Query(q): Query<LangQuery>,
    body: Bytes,
) -> impl IntoResponse {
    if body.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "empty body" }))).into_response();
    }
    let lang = q.lang.unwrap_or_else(|| "en".to_string());
    if !lang_enabled(&state, &lang) {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "speech recognition disabled" }))).into_response();
    }
    let st = state.clone();
    match tokio::task::spawn_blocking(move || transcribe_wav(&st, &lang, &body)).await {
        Ok(Ok(text)) => (StatusCode::OK, Json(json!({ "text": text }))).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// ── Real-time streaming (WebSocket) ──────────────────────────────────────────
#[derive(Deserialize)]
struct StreamQuery { lang: Option<String>, rate: Option<f32> }

async fn stream_handler(
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| stream_session(socket, state, q))
}

async fn stream_session(mut socket: WebSocket, state: AppState, q: StreamQuery) {
    let lang = q.lang.unwrap_or_else(|| "en".to_string());
    if !lang_enabled(&state, &lang) {
        let _ = socket.send(Message::Text(json!({ "error": "speech recognition disabled" }).to_string())).await;
        return;
    }
    let rate = q.rate.unwrap_or(16000.0);
    let (cfg, path) = match resolve_for_lang(&state, &lang) {
        Ok(v) => v,
        Err(e) => { let _ = socket.send(Message::Text(json!({ "error": e.to_string() }).to_string())).await; return; }
    };

    let (audio_tx, audio_rx) = std_mpsc::channel::<Vec<i16>>();
    let (res_tx, mut res_rx) = tok_mpsc::unbounded_channel::<String>();

    // Recognition runs on a dedicated thread (the Whisper state is not friendly
    // to being held across .await).
    let worker = {
        let state = state.clone();
        let lang = lang.clone();
        std::thread::spawn(move || {
            whisper_stream_worker(&state, &path, &lang, rate as u32, &cfg, audio_rx, res_tx);
        })
    };

    loop {
        tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(buf))) => {
                    let samples: Vec<i16> = buf.chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
                    if audio_tx.send(samples).is_err() { break; }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            Some(out) = res_rx.recv() => {
                if socket.send(Message::Text(out)).await.is_err() { break; }
            }
        }
    }
    drop(audio_tx);
    while let Some(out) = res_rx.recv().await {
        let _ = socket.send(Message::Text(out)).await;
    }
    let _ = worker.join();
}

// Whisper isn't streaming: accumulate audio and re-transcribe the (bounded)
// buffer every ~1.5 s, emitting the running text as a partial.
fn whisper_stream_worker(
    state: &AppState, path: &Path, lang: &str, src_rate: u32, cfg: &LangCfg,
    audio_rx: std_mpsc::Receiver<Vec<i16>>, res_tx: tok_mpsc::UnboundedSender<String>,
) {
    let ctx = match whisper_ctx(state, path) { Ok(c) => c, Err(e) => { let _ = res_tx.send(json!({"error": e.to_string()}).to_string()); return; } };
    let mut buf: Vec<i16> = Vec::new();
    let mut since_last = 0usize;
    let chunk = (src_rate as usize * 3) / 2;          // ~1.5 s
    let max_keep = src_rate as usize * 30;            // bound to last 30 s
    while let Ok(samples) = audio_rx.recv() {
        since_last += samples.len();
        buf.extend_from_slice(&samples);
        if buf.len() > max_keep { let drop = buf.len() - max_keep; buf.drain(0..drop); }
        if since_last >= chunk {
            since_last = 0;
            if let Ok(text) = whisper_transcribe(&ctx, &to_f32_16k(&buf, src_rate), lang, cfg) {
                let _ = res_tx.send(json!({ "partial": post_process(state, cfg, lang, &text) }).to_string());
            }
        }
    }
    if let Ok(text) = whisper_transcribe(&ctx, &to_f32_16k(&buf, src_rate), lang, cfg) {
        let _ = res_tx.send(json!({ "text": post_process(state, cfg, lang, &text), "final": true }).to_string());
    }
}

// ── Admin API (role-gated) ───────────────────────────────────────────────────
fn require_admin(headers: &HeaderMap) -> Result<(), Response> {
    let role = headers.get("x-kubuno-user-role").and_then(|v| v.to_str().ok()).unwrap_or("");
    if role == "admin" { Ok(()) }
    else { Err((StatusCode::FORBIDDEN, Json(json!({ "error": "admin only" }))).into_response()) }
}
use axum::response::Response;

async fn admin_catalog(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = require_admin(&headers) { return r; }
    let langs: Vec<_> = catalog::languages().into_iter()
        .map(|(c, l)| json!({ "code": c, "label": l })).collect();
    Json(json!({
        "enabled":   is_enabled(&state),
        "settings":  state.settings.lock().expect("settings").clone(),
        "languages": langs,
        "whisper":   catalog::whisper_models(),
        "installed": installed(&state),
        "config":    state.config.lock().expect("cfg").clone(),
        "downloads": state.downloads.lock().expect("dl").clone(),
    })).into_response()
}

#[derive(Deserialize)]
struct SetEnabledDto { enabled: bool }

async fn admin_set_enabled(State(state): State<AppState>, headers: HeaderMap, Json(dto): Json<SetEnabledDto>) -> Response {
    if let Err(r) = require_admin(&headers) { return r; }
    {
        let mut e = state.enabled.lock().expect("enabled poisoned");
        *e = dto.enabled;
    }
    save_enabled(&state, dto.enabled);
    Json(json!({ "ok": true })).into_response()
}

// All fields except `lang` are optional and merged into the existing entry, so
// the UI can patch one option at a time.
#[derive(Deserialize)]
struct SetCfgDto {
    lang: String,
    engine: Option<String>,
    model: Option<String>,
    enabled: Option<bool>,
    initial_prompt: Option<String>,
    grammar: Option<String>,
    normalize_numbers: Option<bool>,
    punctuation: Option<bool>,
    translate: Option<bool>,
    beam_size: Option<u32>,
    auto_detect: Option<bool>,
}

async fn admin_set_config(State(state): State<AppState>, headers: HeaderMap, Json(dto): Json<SetCfgDto>) -> Response {
    if let Err(r) = require_admin(&headers) { return r; }
    {
        let mut cfg = state.config.lock().expect("cfg");
        let entry = cfg.entry(dto.lang.clone()).or_insert_with(|| LangCfg::whisper(String::new()));
        if let Some(v) = dto.engine            { entry.engine = v; }
        if let Some(v) = dto.model             { entry.model = v; }
        if let Some(v) = dto.enabled           { entry.enabled = v; }
        if let Some(v) = dto.initial_prompt    { entry.initial_prompt = v; }
        if let Some(v) = dto.grammar           { entry.grammar = v; }
        if let Some(v) = dto.normalize_numbers { entry.normalize_numbers = v; }
        if let Some(v) = dto.punctuation       { entry.punctuation = v; }
        if let Some(v) = dto.translate         { entry.translate = v; }
        if let Some(v) = dto.beam_size         { entry.beam_size = v; }
        if let Some(v) = dto.auto_detect       { entry.auto_detect = v; }
        save_config(&state, &cfg);
    }
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct SetSettingsDto {
    silence_ms: Option<u32>,
    sound_threshold: Option<f32>,
    profanity_filter: Option<bool>,
}

async fn admin_set_settings(State(state): State<AppState>, headers: HeaderMap, Json(dto): Json<SetSettingsDto>) -> Response {
    if let Err(r) = require_admin(&headers) { return r; }
    {
        let mut s = state.settings.lock().expect("settings");
        if let Some(v) = dto.silence_ms       { s.silence_ms = v.clamp(500, 30_000); }
        if let Some(v) = dto.sound_threshold  { s.sound_threshold = v.clamp(0.0, 1.0); }
        if let Some(v) = dto.profanity_filter { s.profanity_filter = v; }
        save_settings(&state, &s);
    }
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct DownloadDto { engine: String, model: String }

async fn admin_download(State(state): State<AppState>, headers: HeaderMap, Json(dto): Json<DownloadDto>) -> Response {
    if let Err(r) = require_admin(&headers) { return r; }
    let key = format!("{}/{}", dto.engine, dto.model);
    let Some(url) = catalog::model_url(&dto.engine, &dto.model) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "unknown model" }))).into_response();
    };
    {
        let mut dl = state.downloads.lock().expect("dl");
        if let Some(s) = dl.get(&key) { if s.state == "downloading" {
            return Json(json!({ "ok": true, "already": true })).into_response();
        } }
        dl.insert(key.clone(), DownloadStatus { state: "downloading".into(), received: 0, total: 0, error: None });
    }
    let st = state.clone();
    tokio::spawn(async move { run_download(st, key, url, dto.model).await; });
    Json(json!({ "ok": true })).into_response()
}

async fn run_download(state: AppState, key: String, url: String, model: String) {
    let res = download_inner(&state, &key, &url, &model).await;
    let mut dl = state.downloads.lock().expect("dl");
    match res {
        Ok(()) => { if let Some(s) = dl.get_mut(&key) { s.state = "done".into(); } }
        Err(e) => { dl.insert(key, DownloadStatus { state: "error".into(), received: 0, total: 0, error: Some(e.to_string()) }); }
    }
}

async fn download_inner(state: &AppState, key: &str, url: &str, model: &str) -> anyhow::Result<()> {
    use futures_util::StreamExt;
    let resp = reqwest::get(url).await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    if let Some(s) = state.downloads.lock().expect("dl").get_mut(key) { s.total = total; }

    let tmp = state.data_dir.join(format!("dl-{}.tmp", model));
    std::fs::create_dir_all(&state.data_dir)?;
    let mut file = std::fs::File::create(&tmp)?;
    let mut received: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        std::io::Write::write_all(&mut file, &chunk)?;
        received += chunk.len() as u64;
        if let Some(s) = state.downloads.lock().expect("dl").get_mut(key) { s.received = received; }
    }
    drop(file);

    // Whisper models are a single <id>.bin file.
    std::fs::create_dir_all(whisper_dir(state))?;
    std::fs::rename(&tmp, whisper_dir(state).join(format!("{model}.bin")))?;
    Ok(())
}

async fn admin_delete(State(state): State<AppState>, headers: HeaderMap, AxPath((engine, model)): AxPath<(String, String)>) -> Response {
    if let Err(r) = require_admin(&headers) { return r; }
    let path = match engine.as_str() {
        "whisper" => whisper_dir(&state).join(format!("{model}.bin")),
        _ => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "bad engine" }))).into_response(),
    };
    let res = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
    // Drop any cache + download record.
    state.whisper_cache.lock().expect("wc").remove(&path);
    state.downloads.lock().expect("dl").remove(&format!("{engine}/{model}"));
    match res {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// ── Registration with the core ───────────────────────────────────────────────
fn register_payload() -> serde_json::Value {
    json!({
        "module_id":         "stt",
        "display_name":      "Speech-to-Text",
        "description":       "Reconnaissance vocale auto-hébergée (Whisper)",
        "base_url":          format!("http://{HOST}:{PORT}"),
        "version":           env!("CARGO_PKG_VERSION"),
        "routes":            [{ "method": "*", "path": "/*" }],
        "sidebar_items":     [],
        "subscribed_events": [],
        // Infrastructure interne (alimente la recherche vocale du core) —
        // enregistré pour le routage mais masqué de la liste admin des modules.
        "internal":          true,
    })
}

async fn register_loop(core_url: String, secret: String) {
    let http = reqwest::Client::new();
    let payload = register_payload();
    loop {
        let url = format!("{core_url}/internal/modules/register");
        match http.post(&url).header("X-Internal-Secret", &secret).json(&payload).send().await {
            Ok(r) if r.status().is_success() => { tracing::info!("module stt enregistré auprès du core"); break; }
            Ok(r) => tracing::warn!(status = %r.status(), "register refusé, nouvel essai…"),
            Err(e) => tracing::warn!(error = %e, "register échoué, nouvel essai…"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        let hb = format!("{core_url}/internal/modules/stt/heartbeat");
        if http.post(&hb).header("X-Internal-Secret", &secret).send().await
            .map(|r| r.status().is_success()).unwrap_or(false) { continue; }
        let url = format!("{core_url}/internal/modules/register");
        let _ = http.post(&url).header("X-Internal-Secret", &secret).json(&payload).send().await;
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let module_dir = std::env::var("KUBUNO_MODULE_DIR").map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let data_dir = std::env::var("KUBUNO_DATA_DIR").map(PathBuf::from)
        .unwrap_or_else(|_| module_dir.clone());
    // Models live in the WRITABLE data dir (same filesystem) so the module can
    // download/extract them at runtime — the install dir (/usr/lib) is read-only.
    let state = AppState {
        models_dir: data_dir.join("models"),
        data_dir,
        config:     Arc::new(Mutex::new(Config::new())),
        enabled:    Arc::new(Mutex::new(true)),
        settings:   Arc::new(Mutex::new(GlobalSettings::default())),
        whisper_cache: Arc::new(Mutex::new(HashMap::new())),
        downloads:  Arc::new(Mutex::new(HashMap::new())),
    };
    *state.config.lock().expect("cfg") = load_config(&state);
    *state.enabled.lock().expect("enabled") = load_enabled(&state);
    *state.settings.lock().expect("settings") = load_settings(&state);

    let core_url = std::env::var("KUBUNO_CORE_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    let secret   = std::env::var("KUBUNO_INTERNAL_SECRET").unwrap_or_default();
    tokio::spawn(register_loop(core_url, secret));

    let app = Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/stream", get(stream_handler))
        .route("/transcribe", post(transcribe_handler))
        .route("/admin/catalog", get(admin_catalog))
        .route("/admin/enabled", post(admin_set_enabled))
        .route("/admin/settings", post(admin_set_settings))
        .route("/admin/config", post(admin_set_config))
        .route("/admin/models/download", post(admin_download))
        .route("/admin/models/:engine/:model", delete(admin_delete))
        .layer(DefaultBodyLimit::max(25 * 1024 * 1024))
        .with_state(state);

    let addr = format!("{HOST}:{PORT}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "module stt à l'écoute");
    axum::serve(listener, app).await?;
    Ok(())
}
