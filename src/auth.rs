// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 Andreas Kraxner

//! Anmeldung: Benutzerdatei mit Argon2-Passwort-Hashes, Rollen (admin/benutzer),
//! Sitzungen per Cookie, Sperre nach wiederholten Fehlversuchen.
//!
//! Benutzerdatei (Standard `users.txt`), eine Zeile pro Benutzer:
//! `name:rolle:$argon2id$…[:totp=GEHEIMNIS]` mit rolle = `admin` oder `benutzer`.
//! `totp=` ist das Geheimnis für die freiwillige Zwei-Faktor-Anmeldung (Base32).
//! Verwaltung im Browser unter `/admin` oder über die Unterbefehle
//! `benutzer-anlegen`, `passwort-ändern`, `benutzer-entfernen`, `benutzer-liste`,
//! `zwei-faktor-aus`.

use crate::{i18n, tr};
use anyhow::{bail, Context, Result};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::audit::Audit;
use crate::totp;

pub const COOKIE: &str = "bf_session";
pub const SESSION_TTL: Duration = Duration::from_secs(8 * 3600);
pub const MIN_PASSWORD: usize = 12;
const MAX_FAILS: u32 = 5;
const LOCK_TIME: Duration = Duration::from_secs(5 * 60);
/// So lange gilt ein begonnenes Einrichten der Zwei-Faktor-Anmeldung
const MFA_SETUP_TTL: Duration = Duration::from_secs(10 * 60);
/// Kennung der Kommandozeile im Audit-Log
pub const CLI_ACTOR: &str = "Kommandozeile";

#[derive(Clone)]
pub struct User {
    pub name: String,
    pub admin: bool,
}

#[derive(Clone)]
pub struct UserRec {
    pub admin: bool,
    pub hash: String,
    /// Geheimnis der Zwei-Faktor-Anmeldung (Base32), wenn eingerichtet
    pub totp: Option<String>,
}

#[derive(Serialize)]
pub struct UserEntry {
    pub name: String,
    pub admin: bool,
    /// Zwei-Faktor-Anmeldung eingerichtet
    pub mfa: bool,
}

struct Session {
    user: String,
    expires: Instant,
}

struct Fails {
    count: u32,
    locked_until: Option<Instant>,
}

pub struct Auth {
    users_file: PathBuf,
    sessions: Mutex<HashMap<String, Session>>,
    fails: Mutex<HashMap<String, Fails>>,
    /// Schreibzugriffe auf die Benutzerdatei nacheinander ausführen.
    file_lock: Mutex<()>,
    /// Hash für unbekannte Benutzer, damit die Antwortzeit nichts verrät.
    dummy_hash: String,
    /// zuletzt verwendeter Zeitschritt pro Benutzer: jeder Code gilt nur einmal
    totp_used: Mutex<HashMap<String, u64>>,
    /// begonnenes Einrichten: Benutzer → (Geheimnis, Beginn)
    totp_pending: Mutex<HashMap<String, (String, Instant)>>,
}

pub enum LoginError {
    /// Benutzername oder Passwort falsch
    Invalid,
    /// Passwort richtig, Code aus der App fehlt
    CodeRequired,
    /// Passwort richtig, Code falsch oder schon verwendet
    BadCode,
    Locked(u64),
}

fn random_token() -> String {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("Zufallszahlen nicht verfügbar");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn hash_password(pw: &str) -> Result<String> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| anyhow::anyhow!("{e}"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Argon2::default().hash_password(pw.as_bytes(), &salt).map_err(|e| anyhow::anyhow!("{e}"))?.to_string())
}

fn verify(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash).map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok()).unwrap_or(false)
}

pub fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || "._-@".contains(c))
}

pub fn check_password(pw: &str) -> Result<()> {
    if pw.chars().count() < MIN_PASSWORD {
        bail!(tr!("Das Passwort muss mindestens {MIN_PASSWORD} Zeichen lang sein.", "The password must be at least {MIN_PASSWORD} characters long."));
    }
    Ok(())
}

pub fn read_users(path: &Path) -> Result<BTreeMap<String, UserRec>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e).with_context(|| format!("{} nicht lesbar", path.display())),
    };
    let mut users = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        let (name, admin, rest) = match parts.as_slice() {
            [n, role, h] if *role == "admin" || *role == "benutzer" => (*n, *role == "admin", *h),
            [n, h] => (*n, false, *h),
            _ => continue,
        };
        // Der Argon2-Hash enthält kein „:“; danach folgen optionale Felder „schlüssel=wert“.
        let mut fields = rest.split(':');
        let hash = fields.next().unwrap_or_default().to_string();
        let mut totp = None;
        for f in fields {
            if let Some(v) = f.strip_prefix("totp=") {
                totp = Some(v.to_string()).filter(|v| !v.is_empty());
            }
        }
        users.insert(name.to_string(), UserRec { admin, hash, totp });
    }
    Ok(users)
}

fn write_users(path: &Path, users: &BTreeMap<String, UserRec>) -> Result<()> {
    let mut out = String::from("# Benutzer von Mimi – im Browser unter /admin oder über die Unterbefehle bearbeiten\n");
    for (n, u) in users {
        let totp = u.totp.as_deref().map(|t| format!(":totp={t}")).unwrap_or_default();
        out.push_str(&format!("{n}:{}:{}{totp}\n", if u.admin { "admin" } else { "benutzer" }, u.hash));
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, out)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Passwort abfragen: am Terminal verdeckt, sonst (Skript/Pipe) eine Zeile von der Standardeingabe.
fn read_password(prompt: &str) -> Result<String> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        Ok(rpassword::prompt_password(prompt)?)
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }
}

/// Unterbefehle zur Benutzerverwaltung. Gibt `true` zurück, wenn ein Befehl ausgeführt wurde.
pub fn cli(args: &[String], users_file: &Path, audit: &Audit) -> Result<bool> {
    match args.first().map(String::as_str) {
        Some("benutzer-anlegen") => {
            let name = args.get(1).context("Aufruf: benutzer-anlegen <name> [--admin]")?;
            let admin = args.iter().any(|a| a == "--admin");
            if !valid_name(name) {
                bail!("Ungültiger Name (erlaubt: Buchstaben, Ziffern, . _ - @)");
            }
            let pw = read_password(&format!("Passwort für {name}: "))?;
            check_password(&pw)?;
            let pw2 = read_password("Passwort wiederholen: ")?;
            if pw != pw2 {
                bail!("Die Passwörter stimmen nicht überein.");
            }
            let mut users = read_users(users_file)?;
            // eine eingerichtete Zwei-Faktor-Anmeldung bleibt erhalten
            let totp = users.get(name.as_str()).and_then(|u| u.totp.clone());
            let existed = users.insert(name.clone(), UserRec { admin, hash: hash_password(&pw)?, totp }).is_some();
            write_users(users_file, &users)?;
            audit.log(CLI_ACTOR, if existed { "Benutzer überschrieben" } else { "Benutzer angelegt" }, name, true, if admin { "Admin" } else { "Benutzer" }, "-");
            println!(
                "{} „{name}“ ({}) in {}.",
                if existed { "Benutzer aktualisiert:" } else { "Benutzer angelegt:" },
                if admin { "Admin" } else { "Benutzer" },
                users_file.display()
            );
            Ok(true)
        }
        Some("passwort-ändern") | Some("passwort-aendern") => {
            let name = args.get(1).context("Aufruf: passwort-ändern <name>")?;
            let mut users = read_users(users_file)?;
            if !users.contains_key(name) {
                bail!("Benutzer „{name}“ gibt es nicht – anlegen mit „mimi benutzer-anlegen {name}“.");
            }
            let pw = read_password(&format!("Neues Passwort für {name}: "))?;
            check_password(&pw)?;
            let pw2 = read_password("Passwort wiederholen: ")?;
            if pw != pw2 {
                bail!("Die Passwörter stimmen nicht überein.");
            }
            let hash = hash_password(&pw)?;
            let rec = users.get_mut(name).expect("oben geprüft");
            rec.hash = hash;
            let admin = rec.admin;
            write_users(users_file, &users)?;
            audit.log(CLI_ACTOR, "Passwort gesetzt", name, true, "", "-");
            println!("Passwort für „{name}“ ({}) geändert. Laufende Anmeldungen enden beim nächsten Neustart des Servers.", if admin { "Admin" } else { "Benutzer" });
            Ok(true)
        }
        Some("benutzer-entfernen") => {
            let name = args.get(1).context("Aufruf: benutzer-entfernen <name>")?;
            let mut users = read_users(users_file)?;
            if users.remove(name).is_none() {
                bail!("Benutzer „{name}“ gibt es nicht.");
            }
            write_users(users_file, &users)?;
            audit.log(CLI_ACTOR, "Benutzer gelöscht", name, true, "", "-");
            println!("Benutzer „{name}“ entfernt. Seine Berichte löscht der Server beim nächsten Aufräumen (spätestens in einer Stunde).");
            Ok(true)
        }
        Some("zwei-faktor-aus") => {
            let name = args.get(1).context("Aufruf: zwei-faktor-aus <name>")?;
            let mut users = read_users(users_file)?;
            let rec = users.get_mut(name.as_str()).with_context(|| format!("Benutzer „{name}“ gibt es nicht."))?;
            if rec.totp.take().is_none() {
                println!("„{name}“ hat keine Zwei-Faktor-Anmeldung eingerichtet.");
                return Ok(true);
            }
            write_users(users_file, &users)?;
            audit.log(CLI_ACTOR, "Zwei-Faktor zurückgesetzt", name, true, "", "-");
            println!("Zwei-Faktor-Anmeldung für „{name}“ ausgeschaltet. Anmelden geht jetzt wieder nur mit Passwort.");
            Ok(true)
        }
        Some("benutzer-liste") => {
            let users = read_users(users_file)?;
            if users.is_empty() {
                println!("Keine Benutzer angelegt.");
            }
            for (n, u) in users {
                println!("{n}{}{}", if u.admin { " (Admin)" } else { "" }, if u.totp.is_some() { " [Zwei-Faktor]" } else { "" });
            }
            Ok(true)
        }
        Some("hilfe") | Some("--help") | Some("-h") => {
            println!(
                "Mimi – Macht Inhalte Mühelos Inklusiv\nWord-Berichte barrierefrei machen\n\n  mimi                                   Server starten\n  mimi benutzer-anlegen NAME [--admin]   Benutzer anlegen (oder überschreiben)\n  mimi passwort-ändern NAME              Passwort ändern, Rolle bleibt\n  mimi benutzer-entfernen NAME\n  mimi benutzer-liste\n  mimi zwei-faktor-aus NAME              Zwei-Faktor-Anmeldung zurücksetzen (Handy verloren)\n\nWeitere Benutzer können Admins im Browser unter /admin verwalten."
            );
            Ok(true)
        }
        Some(other) => bail!("Unbekannter Befehl „{other}“ – siehe „mimi hilfe“."),
        None => Ok(false),
    }
}

impl Auth {
    pub fn new(users_file: PathBuf) -> Result<Auth> {
        Ok(Auth {
            users_file,
            sessions: Mutex::new(HashMap::new()),
            fails: Mutex::new(HashMap::new()),
            file_lock: Mutex::new(()),
            dummy_hash: hash_password(&random_token())?,
            totp_used: Mutex::new(HashMap::new()),
            totp_pending: Mutex::new(HashMap::new()),
        })
    }

    pub fn users(&self) -> BTreeMap<String, UserRec> {
        read_users(&self.users_file).unwrap_or_default()
    }

    /// Anmelden. `code`: Einmal-Code aus der App, nötig wenn der Benutzer Zwei-Faktor eingerichtet hat.
    pub async fn login(&self, name: &str, pw: &str, code: Option<&str>) -> Result<String, LoginError> {
        let key = name.to_lowercase();
        {
            let fails = self.fails.lock().await;
            if let Some(Fails { locked_until: Some(t), .. }) = fails.get(&key) {
                if *t > Instant::now() {
                    return Err(LoginError::Locked((*t - Instant::now()).as_secs() / 60 + 1));
                }
            }
        }
        // Benutzerdatei bei jeder Anmeldung neu lesen: Änderungen wirken ohne Neustart.
        let rec = self.users().get(name).cloned();
        let known = rec.is_some();
        let secret = rec.as_ref().and_then(|r| r.totp.clone());
        let hash = rec.map(|r| r.hash).unwrap_or_else(|| self.dummy_hash.clone());
        let pw = pw.to_string();
        let pw_ok = tokio::task::spawn_blocking(move || verify(&pw, &hash)).await.unwrap_or(false) && known;

        // zweiter Faktor
        let mut code_err = None;
        if pw_ok {
            if let Some(secret) = &secret {
                match code.map(str::trim).filter(|c| !c.is_empty()) {
                    // Passwort richtig, Code fehlt: nicht als Fehlversuch zählen
                    None => return Err(LoginError::CodeRequired),
                    Some(c) => match totp::verify(secret, c) {
                        Some(step) => {
                            let mut used = self.totp_used.lock().await;
                            if used.get(name).is_some_and(|last| step <= *last) {
                                code_err = Some(LoginError::BadCode);
                            } else {
                                used.insert(name.to_string(), step);
                            }
                        }
                        None => code_err = Some(LoginError::BadCode),
                    },
                }
            }
        }
        let ok = pw_ok && code_err.is_none();

        let mut fails = self.fails.lock().await;
        if !ok {
            let f = fails.entry(key).or_insert(Fails { count: 0, locked_until: None });
            f.count += 1;
            if f.count >= MAX_FAILS {
                f.count = 0;
                f.locked_until = Some(Instant::now() + LOCK_TIME);
            }
            drop(fails);
            tokio::time::sleep(Duration::from_millis(700)).await;
            return Err(code_err.unwrap_or(LoginError::Invalid));
        }
        fails.remove(&key);
        drop(fails);

        let token = random_token();
        let mut s = self.sessions.lock().await;
        let now = Instant::now();
        s.retain(|_, v| v.expires > now);
        s.insert(token.clone(), Session { user: name.to_string(), expires: now + SESSION_TTL });
        Ok(token)
    }

    pub async fn user_for(&self, token: &str) -> Option<User> {
        let mut s = self.sessions.lock().await;
        let now = Instant::now();
        let sess = s.get_mut(token)?;
        if sess.expires <= now {
            s.remove(token);
            return None;
        }
        // Rolle bei jeder Anfrage aktuell lesen; entfernte Benutzer verlieren die Sitzung sofort.
        match self.users().get(&sess.user) {
            Some(rec) => {
                sess.expires = now + SESSION_TTL;
                Some(User { name: sess.user.clone(), admin: rec.admin })
            }
            None => {
                s.remove(token);
                None
            }
        }
    }

    pub async fn logout(&self, token: &str) {
        self.sessions.lock().await.remove(token);
    }

    /// Alle Sitzungen eines Benutzers beenden (nach Passwortänderung oder Löschung).
    async fn end_sessions(&self, name: &str, except: Option<&str>) {
        self.sessions.lock().await.retain(|t, s| s.user != name || Some(t.as_str()) == except);
    }

    // --- Verwaltung (für /admin) ---

    pub fn list(&self) -> Vec<UserEntry> {
        self.users().into_iter().map(|(name, r)| UserEntry { name, admin: r.admin, mfa: r.totp.is_some() }).collect()
    }

    /// Benutzer anlegen oder ändern. `password = None` lässt das Passwort unverändert.
    pub async fn upsert(&self, actor: &User, actor_token: &str, name: &str, password: Option<&str>, admin: bool) -> Result<()> {
        if !valid_name(name) {
            bail!(i18n::t("Ungültiger Benutzername (erlaubt: Buchstaben, Ziffern, . _ - @; höchstens 64 Zeichen).", "Invalid user name (allowed: letters, digits, . _ - @; at most 64 characters)."));
        }
        if let Some(pw) = password {
            check_password(pw)?;
        }
        let _g = self.file_lock.lock().await;
        let mut users = read_users(&self.users_file)?;
        let hash = match (password, users.get(name)) {
            (Some(pw), _) => {
                let pw = pw.to_string();
                tokio::task::spawn_blocking(move || hash_password(&pw)).await??
            }
            (None, Some(existing)) => existing.hash.clone(),
            (None, None) => bail!(i18n::t("Für neue Benutzer ist ein Passwort nötig.", "New users need a password.")),
        };
        if name == actor.name && !admin {
            bail!(i18n::t("Sie können sich nicht selbst die Admin-Rechte entziehen.", "You cannot remove your own admin rights."));
        }
        // eine eingerichtete Zwei-Faktor-Anmeldung bleibt erhalten
        let totp = users.get(name).and_then(|u| u.totp.clone());
        users.insert(name.to_string(), UserRec { admin, hash, totp });
        write_users(&self.users_file, &users)?;
        drop(_g);
        if password.is_some() {
            let keep = if name == actor.name { Some(actor_token) } else { None };
            self.end_sessions(name, keep).await;
        }
        Ok(())
    }

    pub async fn remove(&self, actor: &User, name: &str) -> Result<()> {
        if name == actor.name {
            bail!(i18n::t("Sie können sich nicht selbst löschen.", "You cannot delete yourself."));
        }
        let _g = self.file_lock.lock().await;
        let mut users = read_users(&self.users_file)?;
        if users.remove(name).is_none() {
            bail!(tr!("Benutzer „{name}“ gibt es nicht.", "User “{name}” does not exist."));
        }
        write_users(&self.users_file, &users)?;
        drop(_g);
        self.end_sessions(name, None).await;
        Ok(())
    }

    // --- Zwei-Faktor-Anmeldung ---

    pub fn has_mfa(&self, name: &str) -> bool {
        self.users().get(name).is_some_and(|u| u.totp.is_some())
    }

    /// Einrichten beginnen: neues Geheimnis, gilt 10 Minuten bis zur Bestätigung mit einem Code.
    pub async fn mfa_begin(&self, name: &str) -> Result<String> {
        if self.has_mfa(name) {
            bail!(i18n::t("Die Zwei-Faktor-Anmeldung ist schon eingerichtet.", "Two-factor login is already set up."));
        }
        let secret = totp::new_secret();
        let mut p = self.totp_pending.lock().await;
        p.retain(|_, (_, t)| t.elapsed() < MFA_SETUP_TTL);
        p.insert(name.to_string(), (secret.clone(), Instant::now()));
        Ok(secret)
    }

    /// Einrichten abschließen: der erste Code aus der App beweist, dass sie richtig eingerichtet ist.
    pub async fn mfa_enable(&self, name: &str, code: &str) -> Result<()> {
        let secret = {
            let p = self.totp_pending.lock().await;
            match p.get(name) {
                Some((s, t)) if t.elapsed() < MFA_SETUP_TTL => s.clone(),
                _ => bail!(i18n::t("Das Einrichten ist abgelaufen – bitte neu beginnen.", "The setup has expired – please start again.")),
            }
        };
        let step = totp::verify(&secret, code).context(i18n::t("Der Code stimmt nicht. Bitte den aktuellen Code aus der App eingeben.", "The code is wrong. Please enter the current code from the app."))?;
        self.set_totp(name, Some(secret)).await?;
        self.totp_pending.lock().await.remove(name);
        self.totp_used.lock().await.insert(name.to_string(), step);
        Ok(())
    }

    /// Selbst ausschalten – nur mit gültigem Code.
    pub async fn mfa_disable(&self, name: &str, code: &str) -> Result<()> {
        let secret = self.users().get(name).and_then(|u| u.totp.clone()).context(i18n::t("Die Zwei-Faktor-Anmeldung ist nicht eingerichtet.", "Two-factor login is not set up."))?;
        totp::verify(&secret, code).context(i18n::t("Der Code stimmt nicht.", "The code is wrong."))?;
        self.set_totp(name, None).await
    }

    /// Admin setzt zurück (z. B. Handy verloren).
    pub async fn mfa_reset(&self, name: &str) -> Result<()> {
        if !self.has_mfa(name) {
            bail!(tr!("„{name}“ hat keine Zwei-Faktor-Anmeldung eingerichtet.", "“{name}” has no two-factor login set up."));
        }
        self.set_totp(name, None).await
    }

    async fn set_totp(&self, name: &str, secret: Option<String>) -> Result<()> {
        let _g = self.file_lock.lock().await;
        let mut users = read_users(&self.users_file)?;
        let rec = users.get_mut(name).with_context(|| tr!("Benutzer „{name}“ gibt es nicht.", "User “{name}” does not exist."))?;
        rec.totp = secret;
        write_users(&self.users_file, &users)
    }
}

pub fn token_from_headers(h: &axum::http::HeaderMap) -> Option<String> {
    h.get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
        .filter(|v| v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn users_file_with_and_without_totp() {
        let dir = std::env::temp_dir().join(format!("leo-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("users.txt");
        // altes Format ohne Zusatzfelder
        std::fs::write(&f, "# Kommentar\nanna:admin:$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA\nbert:benutzer:$argon2id$x:totp=JBSWY3DPEHPK3PXP\n").unwrap();
        let u = read_users(&f).unwrap();
        assert!(u["anna"].admin && u["anna"].totp.is_none());
        assert_eq!(u["anna"].hash, "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA");
        assert_eq!(u["bert"].hash, "$argon2id$x");
        assert_eq!(u["bert"].totp.as_deref(), Some("JBSWY3DPEHPK3PXP"));
        // Schreiben und wieder lesen
        write_users(&f, &u).unwrap();
        let u2 = read_users(&f).unwrap();
        assert_eq!(u2["bert"].totp.as_deref(), Some("JBSWY3DPEHPK3PXP"));
        assert!(u2["anna"].totp.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
