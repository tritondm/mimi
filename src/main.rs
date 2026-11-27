// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 Andreas Kraxner

//! Mimi: Word-Bericht hochladen, prüfen, korrigieren,
//! als PDF/UA (plus DOCX/ODT) exportieren und das PDF prüfen.

mod audit;
mod auth;
mod cleanup;
mod docx;
mod formula;
mod i18n;
mod pdfcheck;
mod pdffix;
mod sanitize;
mod totp;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Extension, Multipart, Path as AxPath, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

const INDEX: &str = include_str!("../static/index.html");
const ADMIN: &str = include_str!("../static/admin.html");
const AUDIT_PAGE: &str = include_str!("../static/audit.html");
const MENU_CSS: &str = include_str!("../static/menu.css");
const MENU_JS: &str = include_str!("../static/menu.js");
/// Sprachumschaltung der Seiten; `__MIMI_DEFAULT_LANG__` wird durch `DEFAULT_LANG` ersetzt
const I18N_JS: &str = include_str!("../static/i18n.js");
const FAVICON_SVG: &str = include_str!("../static/favicon.svg");
const FAVICON_ICO: &[u8] = include_bytes!("../static/favicon.ico");
const TOUCH_ICON: &[u8] = include_bytes!("../static/apple-touch-icon.png");

/// Größte hochladbare Datei (wie `client_max_body_size` in nginx)
const MAX_UPLOAD: usize = 300 << 20;

/// Höchstdauer eines LibreOffice-Aufrufs. Ein Export ruft LibreOffice zweimal auf, dazu kommt
/// veraPDF; nginx wartet insgesamt 900 s (`proxy_read_timeout`).
const LO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

struct AppState {
    base: PathBuf,
    jobs: PathBuf,
    soffice: String,
    /// LibreOffice verträgt keine parallelen Aufrufe mit demselben Profil.
    lo_lock: Mutex<()>,
    auth: auth::Auth,
    /// Cookie nur über HTTPS senden (hinter einem Reverse-Proxy mit TLS: COOKIE_SECURE=1)
    secure_cookie: bool,
    /// automatisches Löschen (JOB_TTL_DAYS)
    policy: cleanup::Policy,
    /// Berichte, die gerade exportiert oder geändert werden – nicht löschen, nicht doppelt bearbeiten
    busy: std::sync::Mutex<std::collections::HashSet<String>>,
    audit: audit::Audit,
}

impl AppState {
    fn busy_ids(&self) -> std::collections::HashSet<String> {
        self.busy.lock().map(|b| b.clone()).unwrap_or_default()
    }
    /// Bericht für Export, Ändern oder Löschen belegen. Läuft schon etwas, gibt es 409:
    /// Zwei Aufrufe würden sonst in denselben Dateien arbeiten.
    fn claim(&self, id: &str) -> Result<BusyGuard<'_>, AppError> {
        let mut b = self.busy.lock().map_err(|_| anyhow::anyhow!(i18n::t("interner Fehler (Sperre)", "internal error (lock)")))?;
        if !b.insert(id.to_string()) {
            return Err(AppError(
                StatusCode::CONFLICT,
                i18n::t(
                    "Der Bericht wird gerade bearbeitet (PDF wird erzeugt oder Änderungen werden gespeichert). Bitte kurz warten und noch einmal versuchen.",
                    "The report is busy (a PDF is being created or changes are being saved). Please wait a moment and try again.",
                )
                .into(),
            ));
        }
        Ok(BusyGuard(self, id.to_string()))
    }
    /// Ereignis eines angemeldeten Benutzers festhalten.
    fn log(&self, user: &auth::User, h: &HeaderMap, action: &str, target: &str, ok: bool, detail: &str) {
        self.audit.log(&user.name, action, target, ok, detail, &audit::client_ip(h));
    }
}

/// Markiert einen Bericht für die Dauer von Export oder Änderung als belegt.
struct BusyGuard<'a>(&'a AppState, String);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut b) = self.0.busy.lock() {
            b.remove(&self.1);
        }
    }
}

type St = State<Arc<AppState>>;

struct AppError(StatusCode, String);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

fn bad_request(msg: &str) -> AppError {
    AppError(StatusCode::BAD_REQUEST, msg.into())
}

/// `spawn_blocking` in der Sprache der laufenden Anfrage.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, AppError> {
    let lang = i18n::current();
    Ok(tokio::task::spawn_blocking(move || i18n::with(lang, f)).await.map_err(|e| anyhow::anyhow!(e))?)
}

fn job_dir(st: &AppState, id: &str, user: &auth::User) -> Result<PathBuf, AppError> {
    let not_found = || AppError(StatusCode::NOT_FOUND, i18n::t("Bericht nicht gefunden – bitte Datei neu hochladen.", "Report not found – please upload the file again.").into());
    let id = uuid::Uuid::parse_str(id).map_err(|_| bad_request(i18n::t("Ungültige Bericht-ID", "Invalid report ID")))?;
    let dir = st.jobs.join(id.to_string());
    if !dir.is_dir() {
        return Err(not_found());
    }
    // Jeder sieht nur seine eigenen Berichte.
    match std::fs::read_to_string(dir.join("owner.txt")) {
        Ok(owner) if owner == user.name => {
            cleanup::touch(&dir);
            Ok(dir)
        }
        _ => Err(not_found()),
    }
}

/// Antwort mit Bericht und Analyse, samt Datum der automatischen Löschung.
fn job_json(st: &AppState, dir: &Path, id: &str, name: &str, analysis: docx::Analysis) -> Json<serde_json::Value> {
    let source = if dir.join("source.txt").exists() { "odt" } else { "docx" };
    Json(json!({ "id": id, "name": name, "analysis": analysis, "delete_after": cleanup::delete_after(dir, &st.policy), "source": source }))
}

fn original_name(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("name.txt")).unwrap_or_else(|_| i18n::t("Bericht", "Report").into())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let base = std::env::current_dir()?;
    let users_file = std::env::var("USERS_FILE").map(PathBuf::from).unwrap_or_else(|_| base.join("users.txt"));
    let data_dir = std::env::var("DATA_DIR").map(PathBuf::from).unwrap_or_else(|_| base.join("data"));
    let audit = audit::Audit::from_env(&data_dir);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if auth::cli(&args, &users_file, &audit)? {
        return Ok(());
    }
    let auth = auth::Auth::new(users_file.clone())?;
    let users = auth.users();
    if !users.values().any(|u| u.admin) {
        eprintln!(
            "Es ist noch kein Admin angelegt. Zuerst ausführen:\n  mimi benutzer-anlegen <name> --admin\n(Benutzerdatei: {})\nWeitere Benutzer kann der Admin dann im Browser unter /admin anlegen.",
            users_file.display()
        );
        std::process::exit(1);
    }
    let i18n_js = I18N_JS.replace("__MIMI_DEFAULT_LANG__", i18n::default_lang().code());
    let secure_cookie = matches!(std::env::var("COOKIE_SECURE").as_deref(), Ok("1") | Ok("true"));
    let jobs = data_dir.join("jobs");
    std::fs::create_dir_all(&jobs)?;
    let soffice = std::env::var("SOFFICE").unwrap_or_else(|_| "soffice".into());
    let policy = cleanup::Policy::from_env();
    let state = Arc::new(AppState { base, jobs, soffice, lo_lock: Mutex::new(()), auth, secure_cookie, policy, busy: Default::default(), audit });

    // Aufräumen: Berichte (wenn eingeschaltet) und Audit-Log – beim Start und danach stündlich
    {
        let st = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                tick.tick().await;
                let st2 = st.clone();
                let res = tokio::task::spawn_blocking(move || {
                    let pruned = st2.audit.prune();
                    let swept = if st2.policy.ttl.is_some() {
                        let users = st2.auth.users().into_keys().collect();
                        Some(cleanup::sweep(&st2.jobs, &st2.policy, &st2.busy_ids(), &users))
                    } else {
                        None
                    };
                    (pruned, swept)
                })
                .await;
                match res {
                    Ok((pruned, swept)) => {
                        if pruned > 0 {
                            println!("Audit-Log: {pruned} Eintrag/Einträge älter als {} Tage entfernt", st.audit.days());
                        }
                        match swept {
                            Some(Ok(s)) if s.removed > 0 => {
                                let text = cleanup::summary(&s, "Löschfrist abgelaufen");
                                println!("{text}");
                                st.audit.log("System", "Automatisch gelöscht", "", true, &text, "-");
                            }
                            Some(Err(e)) => eprintln!("Aufräumen fehlgeschlagen: {e:#}"),
                            _ => {}
                        }
                    }
                    Err(e) => eprintln!("Aufräumen fehlgeschlagen: {e}"),
                }
            }
        });
    }

    let protected = Router::new()
        .route("/api/upload", post(upload).layer(DefaultBodyLimit::max(MAX_UPLOAD)))
        .route("/api/job/{id}", get(job).delete(delete_job))
        .route("/api/job/{id}/media/{*path}", get(media))
        .route("/api/job/{id}/apply", post(apply))
        .route("/api/job/{id}/export", post(export))
        .route("/api/job/{id}/download/{kind}", get(download))
        .route("/api/admin/users", get(admin_list).post(admin_upsert))
        .route("/api/admin/users/{name}", axum::routing::delete(admin_delete))
        .route("/api/admin/users/{name}/mfa-reset", post(admin_mfa_reset))
        .route("/api/admin/audit", get(admin_audit))
        .route("/api/mfa/setup", post(mfa_setup))
        .route("/api/mfa/enable", post(mfa_enable))
        .route("/api/mfa/disable", post(mfa_disable))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    let app = Router::new()
        .route("/", get(|| async { Html(INDEX) }))
        .route("/admin", get(|| async { Html(ADMIN) }))
        .route("/protokoll", get(|| async { Html(AUDIT_PAGE) }))
        .route("/menu.css", get(|| async { ([(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], MENU_CSS) }))
        .route("/favicon.svg", get(|| async { ([(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "public, max-age=86400")], FAVICON_SVG) }))
        .route("/favicon.ico", get(|| async { ([(header::CONTENT_TYPE, "image/x-icon"), (header::CACHE_CONTROL, "public, max-age=86400")], FAVICON_ICO) }))
        .route("/apple-touch-icon.png", get(|| async { ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=86400")], TOUCH_ICON) }))
        .route("/menu.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], MENU_JS) }))
        .route("/i18n.js", get(move || async move { ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], i18n_js) }))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .merge(protected)
        .layer(middleware::from_fn(same_origin))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn(language))
        .layer(DefaultBodyLimit::max(4 << 20))
        .with_state(state.clone());

    let addr = std::env::var("ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("Mimi läuft auf http://{addr}  (Benutzerverwaltung: http://{addr}/admin)");
    println!("Audit-Log: {} (Einträge {} Tage)", audit_path_hint(&data_dir), state.audit.days());
    println!("Standardsprache der Oberfläche: {} (DEFAULT_LANG, umschaltbar in der Kopfzeile)", i18n::default_lang().code());
    match policy.ttl_days() {
        0 => println!("Automatisches Löschen: aus (JOB_TTL_DAYS=0)."),
        d => println!("Automatisches Löschen: {d} Tage nach der letzten Nutzung, ersetzte Berichte nach 1 Tag."),
    }
    match pdfcheck::find_verapdf(&state.base) {
        Some(p) => println!("veraPDF gefunden: {}", p.display()),
        None => println!("veraPDF nicht gefunden – nur Basisprüfung (siehe README)."),
    }
    axum::serve(listener, app).await?;
    Ok(())
}

fn audit_path_hint(data_dir: &Path) -> String {
    std::env::var("AUDIT_FILE").unwrap_or_else(|_| data_dir.join("audit.log").display().to_string())
}

// ---------------------------------------------------------------------------
// Anmeldung und Schutzschichten
// ---------------------------------------------------------------------------

/// Lässt nur angemeldete Benutzer durch und hängt den Benutzer an die Anfrage.
async fn require_auth(State(st): St, mut req: Request, next: Next) -> Response {
    let user = match auth::token_from_headers(req.headers()) {
        Some(t) => st.auth.user_for(&t).await,
        None => None,
    };
    match user {
        Some(u) => {
            req.extensions_mut().insert(u);
            next.run(req).await
        }
        None => AppError(StatusCode::UNAUTHORIZED, i18n::t("Bitte anmelden.", "Please log in.").into()).into_response(),
    }
}

/// Sprache der Anfrage (Cookie `mimi-lang`) für alle Meldungen festlegen.
async fn language(req: Request, next: Next) -> Response {
    let lang = i18n::from_headers(req.headers());
    i18n::scope(lang, next.run(req)).await
}

/// Schreibende Anfragen nur von der eigenen Seite (Schutz gegen fremde Webseiten, CSRF).
async fn same_origin(req: Request, next: Next) -> Response {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
        let origin = req.headers().get(header::ORIGIN).and_then(|h| h.to_str().ok());
        let ok = match origin {
            Some(o) => o.split_once("://").map(|(_, rest)| rest == host).unwrap_or(false),
            // Ohne Origin-Header (z. B. curl, ältere Browser) entscheidet das SameSite-Cookie.
            None => true,
        };
        if !ok {
            return AppError(StatusCode::FORBIDDEN, i18n::t("Anfrage von fremder Seite abgelehnt.", "Request from another site rejected.").into()).into_response();
        }
    }
    next.run(req).await
}

async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    if !h.contains_key(header::CONTENT_SECURITY_POLICY) {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self'; frame-ancestors 'none'; form-action 'self'; base-uri 'none'"),
        );
    }
    res
}

fn session_cookie(st: &AppState, value: &str, max_age: u64) -> String {
    format!(
        "{}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
        auth::COOKIE,
        if st.secure_cookie { "; Secure" } else { "" }
    )
}

#[derive(serde::Deserialize)]
struct LoginReq {
    user: String,
    password: String,
    /// Einmal-Code aus der App (nur bei Zwei-Faktor-Anmeldung)
    #[serde(default)]
    code: Option<String>,
}

async fn login(State(st): St, headers: HeaderMap, Json(req): Json<LoginReq>) -> Result<Response, AppError> {
    let name = req.user.trim();
    let ip = audit::client_ip(&headers);
    let mfa = |msg: &str| (StatusCode::UNAUTHORIZED, Json(json!({ "error": msg, "mfa": true }))).into_response();
    match st.auth.login(name, &req.password, req.code.as_deref()).await {
        Ok(token) => {
            st.audit.log(name, "Anmeldung", "", true, if st.auth.has_mfa(name) { "mit Zwei-Faktor" } else { "" }, &ip);
            let cookie = session_cookie(&st, &token, auth::SESSION_TTL.as_secs());
            Ok(([(header::SET_COOKIE, cookie)], Json(json!({ "user": name }))).into_response())
        }
        Err(auth::LoginError::CodeRequired) => Ok(mfa(i18n::t("Bitte den Code aus Ihrer Authenticator-App eingeben.", "Please enter the code from your authenticator app."))),
        Err(auth::LoginError::BadCode) => {
            st.audit.log(name, "Anmeldung fehlgeschlagen", "", false, "Code falsch oder schon verwendet", &ip);
            Ok(mfa(i18n::t("Der Code stimmt nicht oder wurde schon verwendet. Bitte den aktuellen Code eingeben.", "The code is wrong or has already been used. Please enter the current code.")))
        }
        Err(auth::LoginError::Invalid) => {
            st.audit.log(name, "Anmeldung fehlgeschlagen", "", false, "Benutzername oder Passwort falsch", &ip);
            Err(AppError(StatusCode::UNAUTHORIZED, i18n::t("Benutzername oder Passwort falsch.", "Wrong user name or password.").into()))
        }
        Err(auth::LoginError::Locked(min)) => {
            st.audit.log(name, "Anmeldung gesperrt", "", false, &format!("noch {min} Minute(n)"), &ip);
            Err(AppError(StatusCode::TOO_MANY_REQUESTS, tr!("Zu viele Fehlversuche – bitte in {min} Minute(n) erneut versuchen.", "Too many failed attempts – please try again in {min} minute(s).")))
        }
    }
}

async fn logout(State(st): St, headers: HeaderMap) -> Response {
    if let Some(t) = auth::token_from_headers(&headers) {
        if let Some(u) = st.auth.user_for(&t).await {
            st.log(&u, &headers, "Abmeldung", "", true, "");
        }
        st.auth.logout(&t).await;
    }
    ([(header::SET_COOKIE, session_cookie(&st, "", 0))], Json(json!({ "ok": true }))).into_response()
}

async fn me(State(st): St, headers: HeaderMap) -> Result<Json<serde_json::Value>, AppError> {
    let unauth = || AppError(StatusCode::UNAUTHORIZED, i18n::t("Bitte anmelden.", "Please log in.").into());
    let t = auth::token_from_headers(&headers).ok_or_else(unauth)?;
    let u = st.auth.user_for(&t).await.ok_or_else(unauth)?;
    Ok(Json(json!({ "user": u.name, "admin": u.admin, "ttl_days": st.policy.ttl_days(), "mfa": st.auth.has_mfa(&u.name) })))
}

// ---------------------------------------------------------------------------
// Benutzerverwaltung (nur Admins)
// ---------------------------------------------------------------------------

fn require_admin(user: &auth::User) -> Result<(), AppError> {
    if user.admin {
        Ok(())
    } else {
        Err(AppError(StatusCode::FORBIDDEN, i18n::t("Nur für Admins.", "Admins only.").into()))
    }
}

async fn admin_list(State(st): St, Extension(user): Extension<auth::User>) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    Ok(Json(json!({ "users": st.auth.list(), "min_password": auth::MIN_PASSWORD })))
}

#[derive(serde::Deserialize)]
struct UpsertReq {
    name: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    admin: bool,
}

async fn admin_upsert(State(st): St, Extension(user): Extension<auth::User>, headers: HeaderMap, Json(req): Json<UpsertReq>) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let token = auth::token_from_headers(&headers).unwrap_or_default();
    let pw = req.password.as_deref().filter(|p| !p.is_empty());
    let name = req.name.trim();
    let before = st.auth.users().get(name).cloned();
    st.auth.upsert(&user, &token, name, pw, req.admin).await.map_err(|e| bad_request(&e.to_string()))?;
    let role = if req.admin { "Admin" } else { "Benutzer" };
    match before {
        None => st.log(&user, &headers, "Benutzer angelegt", name, true, role),
        Some(b) => {
            if pw.is_some() {
                st.log(&user, &headers, "Passwort gesetzt", name, true, "");
            }
            if b.admin != req.admin {
                st.log(&user, &headers, "Rolle geändert", name, true, role);
            }
        }
    }
    Ok(Json(json!({ "users": st.auth.list() })))
}

async fn admin_delete(State(st): St, Extension(user): Extension<auth::User>, headers: HeaderMap, AxPath(name): AxPath<String>) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    st.auth.remove(&user, &name).await.map_err(|e| bad_request(&e.to_string()))?;
    // seine Berichte gleich mit
    let s = cleanup::delete_owner(&st.jobs, &name, &st.busy_ids());
    if s.removed > 0 {
        println!("{}", cleanup::summary(&s, "Benutzer gelöscht"));
    }
    st.log(&user, &headers, "Benutzer gelöscht", &name, true, &format!("{} Bericht(e) mitgelöscht", s.removed));
    Ok(Json(json!({ "users": st.auth.list() })))
}

async fn admin_mfa_reset(State(st): St, Extension(user): Extension<auth::User>, headers: HeaderMap, AxPath(name): AxPath<String>) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    st.auth.mfa_reset(&name).await.map_err(|e| bad_request(&e.to_string()))?;
    st.log(&user, &headers, "Zwei-Faktor zurückgesetzt", &name, true, "");
    Ok(Json(json!({ "users": st.auth.list() })))
}

async fn admin_audit(State(st): St, Extension(user): Extension<auth::User>, Query(q): Query<std::collections::HashMap<String, String>>) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let limit = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(2000usize).min(20_000);
    let days = st.audit.days();
    let entries = blocking(move || st.audit.read(limit)).await?;
    Ok(Json(json!({ "entries": entries, "days": days })))
}

// ---------------------------------------------------------------------------
// Zwei-Faktor-Anmeldung (für jeden Benutzer selbst)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct CodeReq {
    code: String,
}

async fn mfa_setup(State(st): St, Extension(user): Extension<auth::User>) -> Result<Json<serde_json::Value>, AppError> {
    let secret = st.auth.mfa_begin(&user.name).await.map_err(|e| bad_request(&e.to_string()))?;
    let uri = totp::uri(&user.name, &secret);
    Ok(Json(json!({ "secret": totp::grouped(&secret), "uri": uri, "qr": totp::qr_svg(&uri), "issuer": totp::ISSUER })))
}

async fn mfa_enable(State(st): St, Extension(user): Extension<auth::User>, headers: HeaderMap, Json(req): Json<CodeReq>) -> Result<Json<serde_json::Value>, AppError> {
    match st.auth.mfa_enable(&user.name, &req.code).await {
        Ok(()) => {
            st.log(&user, &headers, "Zwei-Faktor eingerichtet", &user.name, true, "");
            Ok(Json(json!({ "mfa": true })))
        }
        Err(e) => {
            st.log(&user, &headers, "Zwei-Faktor eingerichtet", &user.name, false, "Code falsch");
            Err(bad_request(&e.to_string()))
        }
    }
}

async fn mfa_disable(State(st): St, Extension(user): Extension<auth::User>, headers: HeaderMap, Json(req): Json<CodeReq>) -> Result<Json<serde_json::Value>, AppError> {
    match st.auth.mfa_disable(&user.name, &req.code).await {
        Ok(()) => {
            st.log(&user, &headers, "Zwei-Faktor ausgeschaltet", &user.name, true, "");
            Ok(Json(json!({ "mfa": false })))
        }
        Err(e) => {
            st.log(&user, &headers, "Zwei-Faktor ausgeschaltet", &user.name, false, "Code falsch");
            Err(bad_request(&e.to_string()))
        }
    }
}

// ---------------------------------------------------------------------------
// Fachliche Endpunkte
// ---------------------------------------------------------------------------

async fn upload(State(st): St, Extension(user): Extension<auth::User>, headers: HeaderMap, mut mp: Multipart) -> Result<Json<serde_json::Value>, AppError> {
    while let Some(field) = mp.next_field().await.map_err(|e| bad_request(&e.to_string()))? {
        if field.name() != Some("file") {
            continue;
        }
        let name = field.file_name().unwrap_or("Bericht.docx").to_string();
        let lower = name.to_lowercase();
        let is_odt = lower.ends_with(".odt");
        if !lower.ends_with(".docx") && !is_odt {
            return Err(bad_request(i18n::t("Bitte eine Word- oder OpenDocument-Datei (.docx oder .odt) hochladen.", "Please upload a Word or OpenDocument file (.docx or .odt).")));
        }
        let raw = field.bytes().await.map_err(|e| bad_request(&e.to_string()))?;
        // Entpackgrenzen (ZIP-Bombe), bevor irgendetwas die Datei entpackt
        let raw2 = raw.clone();
        blocking(move || sanitize::check_zip(&raw2)).await?.map_err(|e| bad_request(&format!("{e:#}")))?;
        let id = uuid::Uuid::new_v4().to_string();
        let dir = st.jobs.join(&id);
        tokio::fs::create_dir_all(&dir).await?;
        // ODT: mit LibreOffice in eine DOCX umwandeln, danach läuft alles wie bei Word
        let data = if is_odt {
            match odt_to_docx(&st, &dir, &raw).await {
                Ok(d) => d,
                Err(e) => {
                    let _ = tokio::fs::remove_dir_all(&dir).await;
                    return Err(e);
                }
            }
        } else {
            raw.to_vec()
        };
        let data2 = data.clone();
        let analysis = match blocking(move || docx::analyze(&data2)).await? {
            Ok(a) => a,
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&dir).await;
                return Err(bad_request(&tr!("Datei konnte nicht gelesen werden: {e:#}", "The file could not be read: {e:#}")));
            }
        };
        tokio::fs::write(dir.join("original.docx"), &data).await?;
        tokio::fs::write(dir.join("current.docx"), &data).await?;
        let stem = name[..name.len() - if is_odt { 4 } else { 5 }].to_string();
        tokio::fs::write(dir.join("name.txt"), &stem).await?;
        tokio::fs::write(dir.join("owner.txt"), &user.name).await?;
        cleanup::touch(&dir);
        let size = if raw.len() < 1_048_576 { format!("{} KB", raw.len().div_ceil(1024)) } else { format!("{:.1} MB", raw.len() as f64 / 1_048_576.0) };
        st.log(&user, &headers, "Bericht hochgeladen", &id, true, &format!("{}, {size}", if is_odt { "ODT" } else { "DOCX" }));
        return Ok(job_json(&st, &dir, &id, &stem, analysis));
    }
    Err(bad_request(i18n::t("Keine Datei übermittelt.", "No file received.")))
}

const ODT_MIME: &[u8] = b"application/vnd.oasis.opendocument.text";

/// ODT prüfen, aufbewahren (`original.odt`, Merker `source.txt`) und in eine DOCX umwandeln.
async fn odt_to_docx(st: &AppState, dir: &Path, odt: &[u8]) -> Result<Vec<u8>, AppError> {
    // Nur echte OpenDocument-Textdokumente an LibreOffice geben
    let mime = docx::read_entry(odt, "mimetype").ok().flatten();
    if mime.as_deref().map(|m| m.trim_ascii()) != Some(ODT_MIME) {
        return Err(bad_request(i18n::t("Die Datei ist kein OpenDocument-Textdokument (.odt).", "The file is not an OpenDocument text document (.odt).")));
    }
    let conv = dir.join("conv");
    tokio::fs::create_dir_all(&conv).await?;
    tokio::fs::write(dir.join("original.odt"), odt).await?;
    // LibreOffice bekommt eine Kopie ohne externe Verweise (Bilder, Abschnitte, Vorlagen)
    let clean = sanitize::odt_without_external(odt).map_err(|e| bad_request(&tr!("Die ODT-Datei konnte nicht gelesen werden: {e:#}", "The ODT file could not be read: {e:#}")))?.0;
    tokio::fs::write(conv.join("original.odt"), &clean).await?;
    drop(clean);
    let out = conv.join("out");
    tokio::fs::create_dir_all(&out).await?;
    soffice_convert(st, &conv.join("original.odt"), &out, "docx:MS Word 2007 XML").await?;
    let docx = tokio::fs::read(out.join("original.docx")).await.map_err(|_| bad_request(i18n::t("LibreOffice konnte die ODT-Datei nicht umwandeln.", "LibreOffice could not convert the ODT file.")))?;
    let _ = tokio::fs::remove_dir_all(&conv).await;
    sanitize::check_zip(&docx).map_err(|e| bad_request(&format!("{e:#}")))?;
    tokio::fs::write(dir.join("source.txt"), "odt").await?;
    Ok(docx)
}

async fn job(State(st): St, Extension(user): Extension<auth::User>, AxPath(id): AxPath<String>) -> Result<Json<serde_json::Value>, AppError> {
    let dir = job_dir(&st, &id, &user)?;
    let data = tokio::fs::read(dir.join("current.docx")).await?;
    let analysis = docx::analyze(&data)?;
    Ok(job_json(&st, &dir, &id, &original_name(&dir), analysis))
}

async fn media(State(st): St, Extension(user): Extension<auth::User>, AxPath((id, path)): AxPath<(String, String)>) -> Result<Response, AppError> {
    let dir = job_dir(&st, &id, &user)?;
    let data = tokio::fs::read(dir.join("current.docx")).await?;
    let entry = format!("word/{}", path.trim_start_matches('/'));
    let bytes = docx::read_entry(&data, &entry)?.ok_or_else(|| AppError(StatusCode::NOT_FOUND, i18n::t("Bild nicht gefunden", "Image not found").into()))?;
    let lower = path.to_lowercase();
    let ct = if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".bmp") {
        "image/bmp"
    } else if lower.ends_with(".tif") || lower.ends_with(".tiff") {
        "image/tiff"
    } else {
        "application/octet-stream"
    };
    // Bilder stammen aus fremden Dateien: als reine Bilder ausliefern, nie als Skript/Seite (z. B. SVG).
    Ok((
        [
            (header::CONTENT_TYPE, ct),
            (header::CACHE_CONTROL, "private, max-age=3600"),
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'; style-src 'unsafe-inline'; sandbox"),
        ],
        bytes,
    )
        .into_response())
}

async fn apply(State(st): St, Extension(user): Extension<auth::User>, headers: HeaderMap, AxPath(id): AxPath<String>, Json(changes): Json<docx::Changes>) -> Result<Json<serde_json::Value>, AppError> {
    let dir = job_dir(&st, &id, &user)?;
    let _busy = st.claim(&id)?;
    let data = tokio::fs::read(dir.join("current.docx")).await?;
    let new = blocking(move || docx::apply(&data, &changes)).await??;
    // Plausibilitätsprüfung: muss wieder lesbar sein, sonst nicht speichern
    let new2 = new.clone();
    let analysis = blocking(move || docx::analyze(&new2))
        .await?
        .map_err(|e| anyhow::anyhow!(tr!("Korrigierte Datei ist fehlerhaft, Änderungen nicht gespeichert: {e:#}", "The corrected file is broken, changes were not saved: {e:#}")))?;
    tokio::fs::write(dir.join("current.docx"), &new).await?;
    let _ = tokio::fs::remove_dir_all(dir.join("out")).await;
    st.log(&user, &headers, "Änderungen gespeichert", &id, true, "");
    Ok(job_json(&st, &dir, &id, &original_name(&dir), analysis))
}

/// Beendet eine Prozessgruppe, wenn sie beim Verlassen noch gesetzt ist.
struct KillGroup(Option<u32>);

impl Drop for KillGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0.and_then(|p| i32::try_from(p).ok()).filter(|p| *p > 1) {
            // SAFETY: killpg hat keine Speicher-Vorbedingungen; die Gruppe gehört unserem Kindprozess.
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
            }
        }
    }
}

async fn soffice_convert(st: &AppState, input: &Path, outdir: &Path, target: &str) -> Result<(), AppError> {
    let profile = st.jobs.parent().unwrap_or(&st.jobs).join("lo-profile");
    // relative Pfade ergäben file://data/… – LibreOffice hielte "data" für einen Rechnernamen und hinge
    let profile = std::path::absolute(&profile).unwrap_or(profile);
    let profile_url = format!("file://{}", profile.display());
    let _guard = st.lo_lock.lock().await;
    let child = tokio::process::Command::new(&st.soffice)
        .arg(format!("-env:UserInstallation={profile_url}"))
        .args(["--headless", "--norestore", "--nolockcheck", "--convert-to", target, "--outdir"])
        .arg(outdir)
        .arg(input)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // eigene Prozessgruppe: soffice startet oosplash und soffice.bin, die sollen mit beendet werden
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow::anyhow!(tr!("LibreOffice konnte nicht gestartet werden ({}): {e}", "LibreOffice could not be started ({}): {e}", st.soffice)))?;
    // Bei Zeitüberschreitung oder wenn der Browser bzw. nginx aufgibt (dann wird dieser Aufruf
    // abgebrochen), LibreOffice beenden. Sonst liefe es weiter, während die Sperre schon frei ist.
    let mut group = KillGroup(child.id());
    let out = tokio::time::timeout(LO_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!(tr!("LibreOffice hat zu lange gebraucht (mehr als {} Minuten)", "LibreOffice took too long (more than {} minutes)", LO_TIMEOUT.as_secs() / 60)))?
        .map_err(|e| anyhow::anyhow!(tr!("LibreOffice ist mit einem Fehler abgebrochen: {e}", "LibreOffice stopped with an error: {e}")))?;
    group.0 = None;
    if !out.status.success() {
        return Err(anyhow::anyhow!(tr!("LibreOffice-Fehler: {}", "LibreOffice error: {}", String::from_utf8_lossy(&out.stderr))).into());
    }
    Ok(())
}

async fn export(State(st): St, Extension(user): Extension<auth::User>, headers: HeaderMap, AxPath(id): AxPath<String>, Query(q): Query<std::collections::HashMap<String, String>>) -> Result<Json<serde_json::Value>, AppError> {
    let lossless = q.get("lossless").is_some_and(|v| v == "1");
    let dir = job_dir(&st, &id, &user)?;
    let _busy = st.claim(&id)?;
    let out = dir.join("out");
    let _ = tokio::fs::remove_dir_all(&out).await;
    tokio::fs::create_dir_all(&out).await?;
    // Für LibreOffice eine vorbereitete Kopie verwenden (Vorlagennamen, Links in
    // Kopf-/Fußzeilen); die herunterladbare DOCX bleibt unverändert.
    let prep_dir = dir.join("prep");
    let _ = tokio::fs::remove_dir_all(&prep_dir).await;
    tokio::fs::create_dir_all(&prep_dir).await?;
    let src = tokio::fs::read(dir.join("current.docx")).await?;
    let (prepared, for_odt, formula_alts) = blocking(move || -> anyhow::Result<_> {
        // Alternativtexte der Formeln aus der korrigierten Datei (nicht aus der Export-Kopie)
        let alts = formula::pdf_alts(&src)?;
        // beide Kopien ohne externe Inhalte, die LibreOffice sonst nachladen würde
        Ok((docx::prepare_for_pdf(&src)?.0, sanitize::docx_without_external(&src)?, alts))
    })
    .await??;
    let input = prep_dir.join("current.docx");
    tokio::fs::write(&input, &prepared).await?;
    drop(prepared);
    let odt_dir = prep_dir.join("odt");
    tokio::fs::create_dir_all(&odt_dir).await?;
    tokio::fs::write(odt_dir.join("current.docx"), &for_odt).await?;
    drop(for_odt);

    // Bilder in voller Auflösung (LibreOffice rechnet sonst auf 300 dpi herunter) und
    // JPEG-Qualität 95 statt 90. Wahlweise verlustfrei: keine JPEG-Artefakte an Schrift
    // und Linien, keine dunklen Säume bei Transparenz – aber deutlich größere Datei.
    let images = if lossless { r#""UseLosslessCompression":{"type":"boolean","value":"true"}"# } else { r#""Quality":{"type":"long","value":"95"}"# };
    let pdf_filter = format!(
        r#"pdf:writer_pdf_Export:{{"UseTaggedPDF":{{"type":"boolean","value":"true"}},"PDFUACompliance":{{"type":"boolean","value":"true"}},"ExportBookmarks":{{"type":"boolean","value":"true"}},"ExportNotes":{{"type":"boolean","value":"false"}},"ReduceImageResolution":{{"type":"boolean","value":"false"}},{images}}}"#
    );
    soffice_convert(&st, &input, &out, &pdf_filter).await?;
    // ODT aus der korrigierten Datei, nicht aus der Export-Kopie: Vorlagennamen und Felder
    // (z. B. in Fußzeilen) bleiben so, wie sie sind – wer mit der ODT weiterarbeitet, braucht sie.
    soffice_convert(&st, &odt_dir.join("current.docx"), &out, "odt").await?;

    let _ = tokio::fs::remove_dir_all(&prep_dir).await;

    let pdf = out.join("current.pdf");
    if !pdf.exists() {
        return Err(anyhow::anyhow!(i18n::t("LibreOffice hat kein PDF erzeugt", "LibreOffice did not create a PDF")).into());
    }
    // Strukturbaum geradeziehen (Rahmen/Textfelder stecken sonst als Div in Absätzen)
    let pdf2 = pdf.clone();
    let (pages, checks) = blocking(move || -> anyhow::Result<_> {
        let fix = pdffix::fix_structure(&pdf2, &formula_alts)?;
        let (pages, mut checks) = pdfcheck::basic_checks(&pdf2)?;
        let mut done = Vec::new();
        if fix.moved > 0 {
            done.push(tr!(
                "{} Bereich(e) aus Rahmen oder Textfeldern aus dem Absatz gelöst, in dem LibreOffice sie abgelegt hatte.",
                "{} area(s) from frames or text boxes moved out of the paragraph LibreOffice had put them in.",
                fix.moved
            ));
        }
        if fix.artifacts > 0 {
            done.push(tr!(
                "{} dekorative Form(en) (z. B. Zierbalken in Kopf-/Fußzeilen) als Artefakt ausgeblendet – LibreOffice hätte sie als Abbildung getaggt.",
                "{} decorative shape(s) (e.g. ornamental bars in headers/footers) hidden as artifacts – LibreOffice would have tagged them as figures.",
                fix.artifacts
            ));
        }
        if fix.formulas > 0 {
            done.push(tr!("{} Formel(n) mit Alternativtext versehen.", "Alternative text added to {} equation(s).", fix.formulas));
        }
        if !done.is_empty() {
            checks.insert(0, pdfcheck::Check { ok: true, severity: String::new(), title: i18n::t("Strukturbaum korrigiert", "Structure tree corrected").into(), detail: done.join(" ") });
        }
        if let Some(in_pdf) = fix.formula_mismatch {
            checks.insert(
                0,
                pdfcheck::Check {
                    ok: false,
                    severity: "warnung".into(),
                    title: i18n::t("Formeln nicht zugeordnet", "Equations not matched").into(),
                    detail: tr!(
                        "Im PDF stehen {in_pdf} Formel(n), in der Word-Datei {}. Mimi setzt die Alternativtexte nur, wenn die Zuordnung eindeutig ist. Bitte in Acrobat oder axesPDF nachtragen.",
                        "The PDF contains {in_pdf} equation(s), the Word file {}. Mimi only sets the alternative texts when they match one to one. Please add them in Acrobat or axesPDF.",
                        formula_alts.len()
                    ),
                },
            );
        }
        Ok((pages, checks))
    })
    .await??;
    let vera = pdfcheck::run_verapdf(pdfcheck::find_verapdf(&st.base), &pdf).await;
    let fails = checks.iter().filter(|c| !c.ok).count();
    let vera_txt = match vera.compliant {
        Some(true) => "veraPDF konform",
        Some(false) => "veraPDF nicht konform",
        None => "ohne veraPDF",
    };
    st.log(&user, &headers, "PDF erzeugt", &id, fails == 0 && vera.compliant != Some(false), &format!("{pages} Seiten, {fails} Problem(e) in der Basisprüfung, {vera_txt}"));
    let report = pdfcheck::PdfReport { pages, checks, verapdf: vera };
    tokio::fs::write(out.join("report.json"), serde_json::to_vec_pretty(&report).unwrap_or_default()).await?;
    Ok(Json(json!(report)))
}

/// Bericht sofort löschen (Knopf „Bericht jetzt löschen“).
async fn delete_job(State(st): St, Extension(user): Extension<auth::User>, headers: HeaderMap, AxPath(id): AxPath<String>) -> Result<Json<serde_json::Value>, AppError> {
    let dir = job_dir(&st, &id, &user)?;
    let _busy = st.claim(&id)?;
    tokio::fs::remove_dir_all(&dir).await?;
    st.log(&user, &headers, "Bericht gelöscht", &id, true, "von Hand");
    Ok(Json(json!({ "deleted": id })))
}

async fn download(State(st): St, Extension(user): Extension<auth::User>, headers: HeaderMap, AxPath((id, kind)): AxPath<(String, String)>) -> Result<Response, AppError> {
    let dir = job_dir(&st, &id, &user)?;
    let stem = original_name(&dir);
    let (file, ct, ext) = match kind.as_str() {
        "docx" => (dir.join("current.docx"), "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "docx"),
        "pdf" => (dir.join("out/current.pdf"), "application/pdf", "pdf"),
        "odt" => (dir.join("out/current.odt"), "application/vnd.oasis.opendocument.text", "odt"),
        _ => return Err(bad_request(i18n::t("Unbekanntes Format", "Unknown format"))),
    };
    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|_| AppError(StatusCode::NOT_FOUND, i18n::t("Datei noch nicht erzeugt – zuerst „PDF erzeugen und prüfen“.", "File not created yet – first use “Create and check PDF”.").into()))?;
    st.log(&user, &headers, "Heruntergeladen", &id, true, ext);
    let fname = tr!("{stem} - barrierefrei.{ext}", "{stem} - accessible.{ext}");
    let ascii: String = fname.chars().map(|c| if c.is_ascii_alphanumeric() || " ._-()".contains(c) { c } else { '_' }).collect();
    let encoded: String = fname.bytes().map(|b| if b.is_ascii_alphanumeric() || b"._-".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let disp = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}");
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, ct)
        .header(header::CONTENT_DISPOSITION, disp)
        .body(Body::from(bytes))
        .unwrap())
}
