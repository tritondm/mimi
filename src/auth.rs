// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 example.org

//! Anmeldung: Benutzerdatei mit Argon2-Passwort-Hashes, Rollen (admin/benutzer),
//! Sitzungen per Cookie, Sperre nach wiederholten Fehlversuchen.
//!
//! Benutzerdatei (Standard `users.txt`), eine Zeile pro Benutzer:
//! `name:rolle:$argon2id$…` mit rolle = `admin` oder `benutzer`.
//! Verwaltung im Browser unter `/admin` oder über die Unterbefehle
//! `benutzer-anlegen`, `benutzer-entfernen`, `benutzer-liste`.

use anyhow::{bail, Context, Result};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub const COOKIE: &str = "bf_session";
pub const SESSION_TTL: Duration = Duration::from_secs(8 * 3600);
pub const MIN_PASSWORD: usize = 12;
const MAX_FAILS: u32 = 5;
const LOCK_TIME: Duration = Duration::from_secs(5 * 60);

#[derive(Clone)]
pub struct User {
    pub name: String,
    pub admin: bool,
}

#[derive(Clone)]
pub struct UserRec {
    pub admin: bool,
    pub hash: String,
}

#[derive(Serialize)]
pub struct UserEntry {
    pub name: String,
    pub admin: bool,
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
}

pub enum LoginError {
    Invalid,
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
        bail!("Das Passwort muss mindestens {MIN_PASSWORD} Zeichen lang sein.");
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
        let (name, admin, hash) = match parts.as_slice() {
            [n, role, h] if *role == "admin" || *role == "benutzer" => (*n, *role == "admin", *h),
            [n, h] => (*n, false, *h),
            _ => continue,
        };
        users.insert(name.to_string(), UserRec { admin, hash: hash.to_string() });
    }
    Ok(users)
}

fn write_users(path: &Path, users: &BTreeMap<String, UserRec>) -> Result<()> {
    let mut out = String::from("# Benutzer von Rampe – im Browser unter /admin oder über die Unterbefehle bearbeiten\n");
    for (n, u) in users {
        out.push_str(&format!("{n}:{}:{}\n", if u.admin { "admin" } else { "benutzer" }, u.hash));
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
pub fn cli(args: &[String], users_file: &Path) -> Result<bool> {
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
            let existed = users.insert(name.clone(), UserRec { admin, hash: hash_password(&pw)? }).is_some();
            write_users(users_file, &users)?;
            println!(
                "{} „{name}“ ({}) in {}.",
                if existed { "Benutzer aktualisiert:" } else { "Benutzer angelegt:" },
                if admin { "Admin" } else { "Benutzer" },
                users_file.display()
            );
            Ok(true)
        }
        Some("benutzer-entfernen") => {
            let name = args.get(1).context("Aufruf: benutzer-entfernen <name>")?;
            let mut users = read_users(users_file)?;
            if users.remove(name).is_none() {
                bail!("Benutzer „{name}“ gibt es nicht.");
            }
            write_users(users_file, &users)?;
            println!("Benutzer „{name}“ entfernt.");
            Ok(true)
        }
        Some("benutzer-liste") => {
            let users = read_users(users_file)?;
            if users.is_empty() {
                println!("Keine Benutzer angelegt.");
            }
            for (n, u) in users {
                println!("{n}{}", if u.admin { " (Admin)" } else { "" });
            }
            Ok(true)
        }
        Some("hilfe") | Some("--help") | Some("-h") => {
            println!(
                "Rampe – Word-Berichte barrierefrei machen\n\n  rampe                                   Server starten\n  rampe benutzer-anlegen NAME [--admin]   Benutzer anlegen / Passwort ändern\n  rampe benutzer-entfernen NAME\n  rampe benutzer-liste\n\nWeitere Benutzer können Admins im Browser unter /admin verwalten."
            );
            Ok(true)
        }
        Some(other) => bail!("Unbekannter Befehl „{other}“ – siehe „rampe hilfe“."),
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
        })
    }

    pub fn users(&self) -> BTreeMap<String, UserRec> {
        read_users(&self.users_file).unwrap_or_default()
    }

    pub async fn login(&self, name: &str, pw: &str) -> Result<String, LoginError> {
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
        let hash = rec.map(|r| r.hash).unwrap_or_else(|| self.dummy_hash.clone());
        let pw = pw.to_string();
        let ok = tokio::task::spawn_blocking(move || verify(&pw, &hash)).await.unwrap_or(false) && known;

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
            return Err(LoginError::Invalid);
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
        self.users().into_iter().map(|(name, r)| UserEntry { name, admin: r.admin }).collect()
    }

    /// Benutzer anlegen oder ändern. `password = None` lässt das Passwort unverändert.
    pub async fn upsert(&self, actor: &User, actor_token: &str, name: &str, password: Option<&str>, admin: bool) -> Result<()> {
        if !valid_name(name) {
            bail!("Ungültiger Benutzername (erlaubt: Buchstaben, Ziffern, . _ - @; höchstens 64 Zeichen).");
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
            (None, None) => bail!("Für neue Benutzer ist ein Passwort nötig."),
        };
        if name == actor.name && !admin {
            bail!("Sie können sich nicht selbst die Admin-Rechte entziehen.");
        }
        users.insert(name.to_string(), UserRec { admin, hash });
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
            bail!("Sie können sich nicht selbst löschen.");
        }
        let _g = self.file_lock.lock().await;
        let mut users = read_users(&self.users_file)?;
        if users.remove(name).is_none() {
            bail!("Benutzer „{name}“ gibt es nicht.");
        }
        write_users(&self.users_file, &users)?;
        drop(_g);
        self.end_sessions(name, None).await;
        Ok(())
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
