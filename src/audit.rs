// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 Mimi contributors

//! Audit-Log für Admins: wer hat wann was getan.
//!
//! Eine JSON-Zeile pro Ereignis in `AUDIT_FILE` (Standard `<DATA_DIR>/audit.log`, Rechte 600),
//! es wird nur angehängt. Einträge älter als `AUDIT_DAYS` Tage (Standard 90) werden beim
//! stündlichen Aufräumen entfernt – das Log enthält selbst personenbezogene Daten.
//!
//! Bewusst **nicht** im Log: Dateinamen und Inhalte der Berichte (nur die Bericht-ID).

use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const DAY: u64 = 24 * 3600;

pub struct Audit {
    path: PathBuf,
    days: u64,
    lock: Mutex<()>,
}

#[derive(Serialize)]
struct Entry<'a> {
    ts: u64,
    actor: &'a str,
    action: &'a str,
    target: &'a str,
    ok: bool,
    detail: &'a str,
    ip: &'a str,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Eingaben von außen (z. B. ein falsch getippter Benutzername) kurz und ohne Steuerzeichen.
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(80).collect()
}

impl Audit {
    pub fn new(path: PathBuf, days: u64) -> Audit {
        Audit { path, days, lock: Mutex::new(()) }
    }

    /// Aus `AUDIT_FILE` und `AUDIT_DAYS`; `data_dir` ist das Datenverzeichnis (`DATA_DIR`).
    pub fn from_env(data_dir: &std::path::Path) -> Audit {
        let path = std::env::var("AUDIT_FILE").map(PathBuf::from).unwrap_or_else(|_| data_dir.join("audit.log"));
        let days = std::env::var("AUDIT_DAYS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(90);
        Audit::new(path, days)
    }

    pub fn days(&self) -> u64 {
        self.days
    }

    /// Ereignis festhalten. Fehler beim Schreiben landen im Journal, brechen aber nichts ab.
    pub fn log(&self, actor: &str, action: &str, target: &str, ok: bool, detail: &str, ip: &str) {
        let (actor, target, detail) = (clean(actor), clean(target), clean(detail));
        let e = Entry { ts: now_secs(), actor: &actor, action, target: &target, ok, detail: &detail, ip };
        let Ok(line) = serde_json::to_string(&e) else { return };
        let _g = self.lock.lock();
        let res = (|| -> std::io::Result<()> {
            if let Some(p) = self.path.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut opts = std::fs::OpenOptions::new();
            opts.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&self.path)?;
            writeln!(f, "{line}")
        })();
        if let Err(e) = res {
            eprintln!("Audit-Log nicht schreibbar ({}): {e}", self.path.display());
        }
    }

    /// Die neuesten Einträge zuerst, höchstens `limit`.
    pub fn read(&self, limit: usize) -> Vec<serde_json::Value> {
        let _g = self.lock.lock();
        let Ok(text) = std::fs::read_to_string(&self.path) else { return Vec::new() };
        text.lines().rev().filter_map(|l| serde_json::from_str(l).ok()).take(limit).collect()
    }

    /// Einträge älter als die Frist entfernen. Gibt die Zahl der entfernten Einträge zurück.
    pub fn prune(&self) -> usize {
        self.prune_at(now_secs())
    }

    fn prune_at(&self, now: u64) -> usize {
        if self.days == 0 {
            return 0;
        }
        let cutoff = now.saturating_sub(self.days * DAY);
        let _g = self.lock.lock();
        let Ok(text) = std::fs::read_to_string(&self.path) else { return 0 };
        let (keep, drop): (Vec<&str>, Vec<&str>) = text.lines().partition(|l| {
            serde_json::from_str::<serde_json::Value>(l).ok().and_then(|v| v.get("ts")?.as_u64()).is_none_or(|ts| ts >= cutoff)
        });
        if drop.is_empty() {
            return 0;
        }
        let tmp = self.path.with_extension("tmp");
        let mut out = keep.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        let ok = std::fs::write(&tmp, out).is_ok() && {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
            }
            std::fs::rename(&tmp, &self.path).is_ok()
        };
        if ok {
            drop.len()
        } else {
            0
        }
    }
}

/// IP-Adresse des Aufrufers: vom Reverse-Proxy (X-Forwarded-For), sonst „-“.
pub fn client_ip(h: &axum::http::HeaderMap) -> String {
    h.get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().chars().filter(|c| c.is_ascii_hexdigit() || ".:".contains(*c)).take(45).collect::<String>())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "-".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_prune() {
        let dir = std::env::temp_dir().join(format!("leo-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Audit::new(dir.join("audit.log"), 90);
        a.log("anna", "Anmeldung", "", true, "", "10.0.0.1");
        a.log("böse\nzeile", "Anmeldung fehlgeschlagen", "", false, "", "-");
        let r = a.read(10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["action"], "Anmeldung fehlgeschlagen");
        // Zeilenumbruch aus fremder Eingabe kann keine Zeile vortäuschen
        assert_eq!(r[0]["actor"], "bösezeile");
        assert_eq!(r[1]["ip"], "10.0.0.1");

        // alter Eintrag fliegt raus
        let mut text = std::fs::read_to_string(dir.join("audit.log")).unwrap();
        text.insert_str(0, "{\"ts\":1000,\"actor\":\"alt\",\"action\":\"Anmeldung\",\"target\":\"\",\"ok\":true,\"detail\":\"\",\"ip\":\"-\"}\n");
        std::fs::write(dir.join("audit.log"), text).unwrap();
        assert_eq!(a.read(10).len(), 3);
        assert_eq!(a.prune(), 1);
        assert_eq!(a.read(10).len(), 2);
        assert_eq!(a.prune(), 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.join("audit.log")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ip_from_proxy() {
        let mut h = axum::http::HeaderMap::new();
        assert_eq!(client_ip(&h), "-");
        h.insert("x-forwarded-for", "203.0.113.7, 10.0.0.1".parse().unwrap());
        assert_eq!(client_ip(&h), "203.0.113.7");
        h.insert("x-forwarded-for", "<script>".parse().unwrap());
        assert_eq!(client_ip(&h), "c");
    }
}
