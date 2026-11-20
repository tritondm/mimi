// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 example.org

//! Rampe: Word-Bericht hochladen, prüfen, korrigieren,
//! als PDF/UA (plus DOCX/ODT) exportieren und das PDF prüfen.

mod auth;
mod docx;
mod pdfcheck;
mod pdffix;

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

struct AppState {
    base: PathBuf,
    jobs: PathBuf,
    soffice: String,
    /// LibreOffice verträgt keine parallelen Aufrufe mit demselben Profil.
    lo_lock: Mutex<()>,
    auth: auth::Auth,
    /// Cookie nur über HTTPS senden (hinter einem Reverse-Proxy mit TLS: COOKIE_SECURE=1)
    secure_cookie: bool,
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

fn job_dir(st: &AppState, id: &str, user: &auth::User) -> Result<PathBuf, AppError> {
    let not_found = || AppError(StatusCode::NOT_FOUND, "Job nicht gefunden – bitte Datei neu hochladen.".into());
    let id = uuid::Uuid::parse_str(id).map_err(|_| bad_request("Ungültige Job-ID"))?;
    let dir = st.jobs.join(id.to_string());
    if !dir.is_dir() {
        return Err(not_found());
    }
    // Jeder sieht nur seine eigenen Berichte.
    match std::fs::read_to_string(dir.join("owner.txt")) {
        Ok(owner) if owner == user.name => Ok(dir),
        _ => Err(not_found()),
    }
}

fn original_name(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("name.txt")).unwrap_or_else(|_| "Bericht".into())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let base = std::env::current_dir()?;
    let users_file = std::env::var("USERS_FILE").map(PathBuf::from).unwrap_or_else(|_| base.join("users.txt"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    if auth::cli(&args, &users_file)? {
        return Ok(());
    }
    let auth = auth::Auth::new(users_file.clone())?;
    let users = auth.users();
    if !users.values().any(|u| u.admin) {
        eprintln!(
            "Es ist noch kein Admin angelegt. Zuerst ausführen:\n  rampe benutzer-anlegen <name> --admin\n(Benutzerdatei: {})\nWeitere Benutzer kann der Admin dann im Browser unter /admin anlegen.",
            users_file.display()
        );
        std::process::exit(1);
    }
    let secure_cookie = matches!(std::env::var("COOKIE_SECURE").as_deref(), Ok("1") | Ok("true"));
    let jobs = std::env::var("DATA_DIR").map(PathBuf::from).unwrap_or_else(|_| base.join("data")).join("jobs");
    std::fs::create_dir_all(&jobs)?;
    let soffice = std::env::var("SOFFICE").unwrap_or_else(|_| "soffice".into());
    let state = Arc::new(AppState { base, jobs, soffice, lo_lock: Mutex::new(()), auth, secure_cookie });

    let protected = Router::new()
        .route("/api/upload", post(upload))
        .route("/api/job/{id}", get(job))
        .route("/api/job/{id}/media/{*path}", get(media))
        .route("/api/job/{id}/apply", post(apply))
        .route("/api/job/{id}/export", post(export))
        .route("/api/job/{id}/download/{kind}", get(download))
        .route("/api/admin/users", get(admin_list).post(admin_upsert))
        .route("/api/admin/users/{name}", axum::routing::delete(admin_delete))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    let app = Router::new()
        .route("/", get(|| async { Html(INDEX) }))
        .route("/admin", get(|| async { Html(ADMIN) }))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .merge(protected)
        .layer(middleware::from_fn(same_origin))
        .layer(middleware::from_fn(security_headers))
        .layer(DefaultBodyLimit::max(300 * 1024 * 1024))
        .with_state(state.clone());

    let addr = std::env::var("ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("Rampe läuft auf http://{addr}  (Benutzerverwaltung: http://{addr}/admin)");
    match pdfcheck::find_verapdf(&state.base) {
        Some(p) => println!("veraPDF gefunden: {}", p.display()),
        None => println!("veraPDF nicht gefunden – nur Basisprüfung (siehe README)."),
    }
    axum::serve(listener, app).await?;
    Ok(())
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
        None => AppError(StatusCode::UNAUTHORIZED, "Bitte anmelden.".into()).into_response(),
    }
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
            return AppError(StatusCode::FORBIDDEN, "Anfrage von fremder Seite abgelehnt.".into()).into_response();
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
}

async fn login(State(st): St, Json(req): Json<LoginReq>) -> Result<Response, AppError> {
    match st.auth.login(req.user.trim(), &req.password).await {
        Ok(token) => {
            let cookie = session_cookie(&st, &token, auth::SESSION_TTL.as_secs());
            Ok(([(header::SET_COOKIE, cookie)], Json(json!({ "user": req.user.trim() }))).into_response())
        }
        Err(auth::LoginError::Invalid) => Err(AppError(StatusCode::UNAUTHORIZED, "Benutzername oder Passwort falsch.".into())),
        Err(auth::LoginError::Locked(min)) => Err(AppError(StatusCode::TOO_MANY_REQUESTS, format!("Zu viele Fehlversuche – bitte in {min} Minute(n) erneut versuchen."))),
    }
}

async fn logout(State(st): St, headers: HeaderMap) -> Response {
    if let Some(t) = auth::token_from_headers(&headers) {
        st.auth.logout(&t).await;
    }
    ([(header::SET_COOKIE, session_cookie(&st, "", 0))], Json(json!({ "ok": true }))).into_response()
}

async fn me(State(st): St, headers: HeaderMap) -> Result<Json<serde_json::Value>, AppError> {
    let t = auth::token_from_headers(&headers).ok_or_else(|| AppError(StatusCode::UNAUTHORIZED, "Bitte anmelden.".into()))?;
    let u = st.auth.user_for(&t).await.ok_or_else(|| AppError(StatusCode::UNAUTHORIZED, "Bitte anmelden.".into()))?;
    Ok(Json(json!({ "user": u.name, "admin": u.admin })))
}

// ---------------------------------------------------------------------------
// Benutzerverwaltung (nur Admins)
// ---------------------------------------------------------------------------

fn require_admin(user: &auth::User) -> Result<(), AppError> {
    if user.admin {
        Ok(())
    } else {
        Err(AppError(StatusCode::FORBIDDEN, "Nur für Admins.".into()))
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
    st.auth.upsert(&user, &token, req.name.trim(), pw, req.admin).await.map_err(|e| bad_request(&e.to_string()))?;
    Ok(Json(json!({ "users": st.auth.list() })))
}

async fn admin_delete(State(st): St, Extension(user): Extension<auth::User>, AxPath(name): AxPath<String>) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    st.auth.remove(&user, &name).await.map_err(|e| bad_request(&e.to_string()))?;
    Ok(Json(json!({ "users": st.auth.list() })))
}

// ---------------------------------------------------------------------------
// Fachliche Endpunkte
// ---------------------------------------------------------------------------

async fn upload(State(st): St, Extension(user): Extension<auth::User>, mut mp: Multipart) -> Result<Json<serde_json::Value>, AppError> {
    while let Some(field) = mp.next_field().await.map_err(|e| bad_request(&e.to_string()))? {
        if field.name() != Some("file") {
            continue;
        }
        let name = field.file_name().unwrap_or("Bericht.docx").to_string();
        if !name.to_lowercase().ends_with(".docx") {
            return Err(bad_request("Bitte eine Word-Datei (.docx) hochladen."));
        }
        let data = field.bytes().await.map_err(|e| bad_request(&e.to_string()))?;
        let analysis = docx::analyze(&data).map_err(|e| bad_request(&format!("Datei konnte nicht gelesen werden: {e:#}")))?;
        let id = uuid::Uuid::new_v4().to_string();
        let dir = st.jobs.join(&id);
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(dir.join("original.docx"), &data).await?;
        tokio::fs::write(dir.join("current.docx"), &data).await?;
        let stem = name.trim_end_matches(".docx").trim_end_matches(".DOCX").to_string();
        tokio::fs::write(dir.join("name.txt"), &stem).await?;
        tokio::fs::write(dir.join("owner.txt"), &user.name).await?;
        return Ok(Json(json!({ "id": id, "name": stem, "analysis": analysis })));
    }
    Err(bad_request("Keine Datei übermittelt."))
}

async fn job(State(st): St, Extension(user): Extension<auth::User>, AxPath(id): AxPath<String>) -> Result<Json<serde_json::Value>, AppError> {
    let dir = job_dir(&st, &id, &user)?;
    let data = tokio::fs::read(dir.join("current.docx")).await?;
    let analysis = docx::analyze(&data)?;
    Ok(Json(json!({ "id": id, "name": original_name(&dir), "analysis": analysis })))
}

async fn media(State(st): St, Extension(user): Extension<auth::User>, AxPath((id, path)): AxPath<(String, String)>) -> Result<Response, AppError> {
    let dir = job_dir(&st, &id, &user)?;
    let data = tokio::fs::read(dir.join("current.docx")).await?;
    let entry = format!("word/{}", path.trim_start_matches('/'));
    let bytes = docx::read_entry(&data, &entry)?.ok_or_else(|| AppError(StatusCode::NOT_FOUND, "Bild nicht gefunden".into()))?;
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

async fn apply(State(st): St, Extension(user): Extension<auth::User>, AxPath(id): AxPath<String>, Json(changes): Json<docx::Changes>) -> Result<Json<serde_json::Value>, AppError> {
    let dir = job_dir(&st, &id, &user)?;
    let data = tokio::fs::read(dir.join("current.docx")).await?;
    let new = tokio::task::spawn_blocking(move || docx::apply(&data, &changes)).await.map_err(|e| anyhow::anyhow!(e))??;
    // Plausibilitätsprüfung: muss wieder lesbar sein, sonst nicht speichern
    let analysis = docx::analyze(&new).map_err(|e| anyhow::anyhow!("Korrigierte Datei ist fehlerhaft, Änderungen nicht gespeichert: {e:#}"))?;
    tokio::fs::write(dir.join("current.docx"), &new).await?;
    let _ = tokio::fs::remove_dir_all(dir.join("out")).await;
    Ok(Json(json!({ "id": id, "name": original_name(&dir), "analysis": analysis })))
}

async fn soffice_convert(st: &AppState, input: &Path, outdir: &Path, target: &str) -> Result<(), AppError> {
    let profile = st.jobs.parent().unwrap_or(&st.jobs).join("lo-profile");
    // relative Pfade ergäben file://data/… – LibreOffice hielte "data" für einen Rechnernamen und hinge
    let profile = std::path::absolute(&profile).unwrap_or(profile);
    let profile_url = format!("file://{}", profile.display());
    let _guard = st.lo_lock.lock().await;
    let fut = tokio::process::Command::new(&st.soffice)
        .arg(format!("-env:UserInstallation={profile_url}"))
        .args(["--headless", "--norestore", "--nolockcheck", "--convert-to", target, "--outdir"])
        .arg(outdir)
        .arg(input)
        .output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(600), fut)
        .await
        .map_err(|_| anyhow::anyhow!("LibreOffice hat zu lange gebraucht"))?
        .map_err(|e| anyhow::anyhow!("LibreOffice konnte nicht gestartet werden ({}): {e}", st.soffice))?;
    if !out.status.success() {
        return Err(anyhow::anyhow!("LibreOffice-Fehler: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(())
}

async fn export(State(st): St, Extension(user): Extension<auth::User>, AxPath(id): AxPath<String>, Query(q): Query<std::collections::HashMap<String, String>>) -> Result<Json<serde_json::Value>, AppError> {
    let lossless = q.get("lossless").is_some_and(|v| v == "1");
    let dir = job_dir(&st, &id, &user)?;
    let out = dir.join("out");
    let _ = tokio::fs::remove_dir_all(&out).await;
    tokio::fs::create_dir_all(&out).await?;
    // Für LibreOffice eine vorbereitete Kopie verwenden (Vorlagennamen, Links in
    // Kopf-/Fußzeilen); die herunterladbare DOCX bleibt unverändert.
    let prep_dir = dir.join("prep");
    let _ = tokio::fs::remove_dir_all(&prep_dir).await;
    tokio::fs::create_dir_all(&prep_dir).await?;
    let src = tokio::fs::read(dir.join("current.docx")).await?;
    let (prepared, _notes) = tokio::task::spawn_blocking(move || docx::prepare_for_pdf(&src)).await.map_err(|e| anyhow::anyhow!(e))??;
    let input = prep_dir.join("current.docx");
    tokio::fs::write(&input, &prepared).await?;
    drop(prepared);

    // Bilder in voller Auflösung (LibreOffice rechnet sonst auf 300 dpi herunter) und
    // JPEG-Qualität 95 statt 90. Wahlweise verlustfrei: keine JPEG-Artefakte an Schrift
    // und Linien, keine dunklen Säume bei Transparenz – aber deutlich größere Datei.
    let images = if lossless { r#""UseLosslessCompression":{"type":"boolean","value":"true"}"# } else { r#""Quality":{"type":"long","value":"95"}"# };
    let pdf_filter = format!(
        r#"pdf:writer_pdf_Export:{{"UseTaggedPDF":{{"type":"boolean","value":"true"}},"PDFUACompliance":{{"type":"boolean","value":"true"}},"ExportBookmarks":{{"type":"boolean","value":"true"}},"ExportNotes":{{"type":"boolean","value":"false"}},"ReduceImageResolution":{{"type":"boolean","value":"false"}},{images}}}"#
    );
    soffice_convert(&st, &input, &out, &pdf_filter).await?;
    soffice_convert(&st, &input, &out, "odt").await?;

    let _ = tokio::fs::remove_dir_all(&prep_dir).await;

    let pdf = out.join("current.pdf");
    if !pdf.exists() {
        return Err(anyhow::anyhow!("LibreOffice hat kein PDF erzeugt").into());
    }
    // Strukturbaum geradeziehen (Rahmen/Textfelder stecken sonst als Div in Absätzen)
    let pdf2 = pdf.clone();
    let (pages, checks) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let fix = pdffix::fix_structure(&pdf2)?;
        let (pages, mut checks) = pdfcheck::basic_checks(&pdf2)?;
        let mut done = Vec::new();
        if fix.moved > 0 {
            done.push(format!("{} Bereich(e) aus Rahmen oder Textfeldern aus dem Absatz gelöst, in dem LibreOffice sie abgelegt hatte.", fix.moved));
        }
        if fix.artifacts > 0 {
            done.push(format!("{} dekorative Form(en) (z. B. Zierbalken in Kopf-/Fußzeilen) als Artefakt ausgeblendet – LibreOffice hätte sie als Abbildung getaggt.", fix.artifacts));
        }
        if !done.is_empty() {
            checks.insert(0, pdfcheck::Check { ok: true, severity: String::new(), title: "Strukturbaum korrigiert".into(), detail: done.join(" ") });
        }
        Ok((pages, checks))
    })
    .await
    .map_err(|e| anyhow::anyhow!(e))??;
    let vera = pdfcheck::run_verapdf(pdfcheck::find_verapdf(&st.base), &pdf).await;
    let report = pdfcheck::PdfReport { pages, checks, verapdf: vera };
    tokio::fs::write(out.join("report.json"), serde_json::to_vec_pretty(&report).unwrap_or_default()).await?;
    Ok(Json(json!(report)))
}

async fn download(State(st): St, Extension(user): Extension<auth::User>, AxPath((id, kind)): AxPath<(String, String)>) -> Result<Response, AppError> {
    let dir = job_dir(&st, &id, &user)?;
    let stem = original_name(&dir);
    let (file, ct, ext) = match kind.as_str() {
        "docx" => (dir.join("current.docx"), "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "docx"),
        "pdf" => (dir.join("out/current.pdf"), "application/pdf", "pdf"),
        "odt" => (dir.join("out/current.odt"), "application/vnd.oasis.opendocument.text", "odt"),
        _ => return Err(bad_request("Unbekanntes Format")),
    };
    let bytes = tokio::fs::read(&file).await.map_err(|_| AppError(StatusCode::NOT_FOUND, "Datei noch nicht erzeugt – zuerst „PDF erzeugen und prüfen“.".into()))?;
    let fname = format!("{stem} - barrierefrei.{ext}");
    let ascii: String = fname.chars().map(|c| if c.is_ascii_alphanumeric() || " ._-()".contains(c) { c } else { '_' }).collect();
    let encoded: String = fname.bytes().map(|b| if b.is_ascii_alphanumeric() || b"._-".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let disp = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}");
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, ct)
        .header(header::CONTENT_DISPOSITION, disp)
        .body(Body::from(bytes))
        .unwrap())
}
