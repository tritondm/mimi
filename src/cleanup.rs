// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 example.org

//! Automatisches Löschen hochgeladener Berichte.
//!
//! - **Löschfrist:** ein Bericht wird `JOB_TTL_DAYS` Tage (Standard 30) nach seiner letzten
//!   Nutzung gelöscht. Nutzung = jeder Zugriff (Öffnen, Speichern, Export, Download); Leo
//!   schreibt dann den Zeitpunkt in die Datei `last_used` des Berichts.
//! - **Ersetzte Berichte:** Die Oberfläche zeigt immer nur den zuletzt bearbeiteten Bericht.
//!   Ältere Berichte desselben Benutzers sind nicht mehr erreichbar und werden nach einem Tag
//!   ohne Nutzung gelöscht (Puffer, falls jemand in zwei Browser-Tabs arbeitet).
//! - Berichte ohne Besitzer (abgebrochenes Hochladen) werden nach einem Tag gelöscht.
//! - `JOB_TTL_DAYS=0` schaltet das automatische Löschen ab.
//!
//! Ein Bericht, der gerade exportiert wird, bleibt immer stehen.

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LAST_USED: &str = "last_used";
const DAY: u64 = 24 * 3600;
/// Frist für ersetzte Berichte und Reste abgebrochener Uploads
const GRACE: Duration = Duration::from_secs(DAY);
/// `last_used` höchstens einmal pro Minute schreiben (Vorschaubilder kommen in Serie)
const TOUCH_EVERY: u64 = 60;

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Löschfrist nach der letzten Nutzung; `None` = automatisches Löschen aus
    pub ttl: Option<Duration>,
}

impl Policy {
    pub fn from_env() -> Policy {
        let days = std::env::var("JOB_TTL_DAYS").ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(30);
        Policy { ttl: (days > 0).then(|| Duration::from_secs(days * DAY)) }
    }
    pub fn ttl_days(&self) -> u64 {
        self.ttl.map(|d| d.as_secs() / DAY).unwrap_or(0)
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn mtime_secs(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Letzte Nutzung (Unix-Sekunden). Ohne `last_used` (Berichte von vor dieser Funktion):
/// jüngste Änderung einer Datei im Bericht.
pub fn last_used(dir: &Path) -> u64 {
    if let Some(t) = std::fs::read_to_string(dir.join(LAST_USED)).ok().and_then(|s| s.trim().parse().ok()) {
        return t;
    }
    let mut newest = mtime_secs(dir).unwrap_or(0);
    for sub in [dir.to_path_buf(), dir.join("out")] {
        if let Ok(rd) = std::fs::read_dir(&sub) {
            for e in rd.flatten() {
                newest = newest.max(mtime_secs(&e.path()).unwrap_or(0));
            }
        }
    }
    newest
}

/// Nutzung vermerken.
pub fn touch(dir: &Path) {
    let now = now_secs();
    let prev = std::fs::read_to_string(dir.join(LAST_USED)).ok().and_then(|s| s.trim().parse::<u64>().ok());
    if prev.is_none_or(|p| now.saturating_sub(p) >= TOUCH_EVERY) {
        let _ = std::fs::write(dir.join(LAST_USED), now.to_string());
    }
}

/// Datum der automatischen Löschung (JJJJ-MM-TT, UTC), wenn der Bericht nicht mehr genutzt wird.
pub fn delete_after(dir: &Path, policy: &Policy) -> Option<String> {
    let ttl = policy.ttl?;
    Some(civil_date(last_used(dir) + ttl.as_secs()))
}

/// Unix-Sekunden → Datum (Algorithmus von Howard Hinnant, ohne Zusatzbibliothek).
fn civil_date(secs: u64) -> String {
    let z = (secs / DAY) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            _ => e.metadata().map(|m| m.len()).unwrap_or(0),
        })
        .sum()
}

/// Ergebnis eines Aufräumlaufs.
#[derive(Default, Debug, PartialEq)]
pub struct Swept {
    pub removed: usize,
    pub bytes: u64,
}

fn remove(dir: &Path, out: &mut Swept) {
    let size = dir_size(dir);
    if std::fs::remove_dir_all(dir).is_ok() {
        out.removed += 1;
        out.bytes += size;
    }
}

/// Ein Aufräumlauf. `busy`: IDs von Berichten, die gerade exportiert werden.
pub fn sweep(jobs: &Path, policy: &Policy, busy: &HashSet<String>) -> Result<Swept> {
    sweep_at(jobs, policy, busy, now_secs())
}

fn sweep_at(jobs: &Path, policy: &Policy, busy: &HashSet<String>, now: u64) -> Result<Swept> {
    let mut out = Swept::default();
    let Some(ttl) = policy.ttl else { return Ok(out) };
    let age = |t: u64| now.saturating_sub(t);

    // (Besitzer → [(Zeitpunkt, Ordner)])
    let mut by_owner: HashMap<String, Vec<(u64, PathBuf)>> = HashMap::new();
    for e in std::fs::read_dir(jobs)?.flatten() {
        let dir = e.path();
        let id = e.file_name().to_string_lossy().into_owned();
        if !dir.is_dir() || uuid::Uuid::parse_str(&id).is_err() || busy.contains(&id) {
            continue;
        }
        let t = last_used(&dir);
        match std::fs::read_to_string(dir.join("owner.txt")) {
            Ok(owner) => by_owner.entry(owner).or_default().push((t, dir)),
            // abgebrochenes Hochladen
            Err(_) if age(t) >= GRACE.as_secs() => remove(&dir, &mut out),
            Err(_) => {}
        }
    }
    for (_, mut list) in by_owner {
        // jüngster Bericht zuerst
        list.sort_by(|a, b| b.0.cmp(&a.0));
        for (i, (t, dir)) in list.iter().enumerate() {
            let limit = if i == 0 { ttl } else { GRACE.min(ttl) };
            if age(*t) >= limit.as_secs() {
                remove(dir, &mut out);
            }
        }
    }
    Ok(out)
}

/// Alle Berichte eines Benutzers löschen (beim Löschen des Benutzers).
pub fn delete_owner(jobs: &Path, owner: &str, busy: &HashSet<String>) -> Swept {
    let mut out = Swept::default();
    let Ok(rd) = std::fs::read_dir(jobs) else { return out };
    for e in rd.flatten() {
        let id = e.file_name().to_string_lossy().into_owned();
        if busy.contains(&id) {
            continue;
        }
        let dir = e.path();
        if std::fs::read_to_string(dir.join("owner.txt")).is_ok_and(|o| o == owner) {
            remove(&dir, &mut out);
        }
    }
    out
}

/// „3 Bericht(e) gelöscht (412 MB)“ – ohne Dateinamen.
pub fn summary(s: &Swept, why: &str) -> String {
    format!("{} Bericht(e) gelöscht ({:.0} MB, {why})", s.removed, s.bytes as f64 / 1_048_576.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("leo-cleanup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Bericht anlegen, zuletzt genutzt vor `days` Tagen (bezogen auf `now`).
    fn job(jobs: &Path, owner: Option<&str>, now: u64, days: f64) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let dir = jobs.join(&id);
        std::fs::create_dir_all(dir.join("out")).unwrap();
        std::fs::write(dir.join("current.docx"), vec![0u8; 1000]).unwrap();
        if let Some(o) = owner {
            std::fs::write(dir.join("owner.txt"), o).unwrap();
        }
        std::fs::write(dir.join(LAST_USED), (now - (days * DAY as f64) as u64).to_string()).unwrap();
        id
    }

    const NOW: u64 = 1_800_000_000;
    const P30: Policy = Policy { ttl: Some(Duration::from_secs(30 * DAY)) };

    #[test]
    fn deletes_by_rules() {
        let jobs = tmp("rules");
        let anna_new = job(&jobs, Some("anna"), NOW, 0.1);
        let anna_old_fresh = job(&jobs, Some("anna"), NOW, 0.5); // ersetzt, aber < 1 Tag
        let anna_old = job(&jobs, Some("anna"), NOW, 3.0); // ersetzt, > 1 Tag
        let bert_only = job(&jobs, Some("bert"), NOW, 20.0); // einziger, < 30 Tage
        let carl_only = job(&jobs, Some("carl"), NOW, 31.0); // einziger, > 30 Tage
        let orphan = job(&jobs, None, NOW, 2.0);
        let busy_old = job(&jobs, Some("dora"), NOW, 40.0);
        let busy: HashSet<String> = [busy_old.clone()].into();

        let s = sweep_at(&jobs, &P30, &busy, NOW).unwrap();
        let left: HashSet<String> = std::fs::read_dir(&jobs).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(s.removed, 3);
        assert!(s.bytes >= 3000);
        for id in [&anna_new, &anna_old_fresh, &bert_only, &busy_old] {
            assert!(left.contains(id), "{id} hätte bleiben sollen");
        }
        for id in [&anna_old, &carl_only, &orphan] {
            assert!(!left.contains(id), "{id} hätte gelöscht werden sollen");
        }
        let _ = std::fs::remove_dir_all(&jobs);
    }

    #[test]
    fn off_deletes_nothing() {
        let jobs = tmp("off");
        job(&jobs, Some("anna"), NOW, 400.0);
        job(&jobs, Some("anna"), NOW, 300.0);
        let s = sweep_at(&jobs, &Policy { ttl: None }, &HashSet::new(), NOW).unwrap();
        assert_eq!(s, Swept::default());
        let _ = std::fs::remove_dir_all(&jobs);
    }

    #[test]
    fn delete_owner_only_theirs() {
        let jobs = tmp("owner");
        job(&jobs, Some("anna"), NOW, 0.0);
        job(&jobs, Some("anna"), NOW, 5.0);
        let bert = job(&jobs, Some("bert"), NOW, 0.0);
        assert_eq!(delete_owner(&jobs, "anna", &HashSet::new()).removed, 2);
        assert!(jobs.join(&bert).is_dir());
        let _ = std::fs::remove_dir_all(&jobs);
    }

    #[test]
    fn dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(1_800_000_000), "2027-01-15");
        assert_eq!(civil_date(951_782_400), "2000-02-29");
    }

    #[test]
    fn touch_and_fallback() {
        let jobs = tmp("touch");
        let dir = jobs.join("x");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("current.docx"), b"x").unwrap();
        // ohne last_used: Änderungszeit der Dateien
        assert!(now_secs() - last_used(&dir) < 5);
        touch(&dir);
        let t = std::fs::read_to_string(dir.join(LAST_USED)).unwrap();
        assert!(now_secs() - t.parse::<u64>().unwrap() < 5);
        let _ = std::fs::remove_dir_all(&jobs);
    }
}
