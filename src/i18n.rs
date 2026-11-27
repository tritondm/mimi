// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 example.org

//! Sprache der Oberfläche: Deutsch (Standard) oder Englisch.
//!
//! Der Browser schickt die Wahl im Cookie `mimi-lang` (`de` oder `en`); ohne Cookie gilt
//! `DEFAULT_LANG` (Standard `de`). Eine Schicht in `main.rs` legt die Sprache für die ganze
//! Anfrage fest (`scope`), für Arbeit in `spawn_blocking` reicht `with` sie weiter.
//! Texte stehen nebeneinander: `tr!("Deutsch {n}", "English {n}")` bzw. `t("…", "…")`.
//! Server-Ausgaben (Konsole, Audit-Log) bleiben deutsch.

use std::cell::Cell;
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lang {
    #[default]
    De,
    En,
}

pub const COOKIE: &str = "mimi-lang";

impl Lang {
    pub fn parse(s: &str) -> Option<Lang> {
        match s.trim().to_ascii_lowercase().as_str() {
            "de" => Some(Lang::De),
            "en" => Some(Lang::En),
            _ => None,
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Lang::De => "de",
            Lang::En => "en",
        }
    }
}

/// Sprache ohne Cookie: `DEFAULT_LANG` (`de` oder `en`), sonst Deutsch.
pub fn default_lang() -> Lang {
    static D: OnceLock<Lang> = OnceLock::new();
    *D.get_or_init(|| std::env::var("DEFAULT_LANG").ok().and_then(|v| Lang::parse(&v)).unwrap_or_default())
}

/// Sprache aus dem Cookie `mimi-lang`, sonst die Standardsprache.
pub fn from_headers(h: &axum::http::HeaderMap) -> Lang {
    h.get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .and_then(|(_, v)| Lang::parse(v))
        .unwrap_or_else(default_lang)
}

tokio::task_local! {
    static TASK: Lang;
}

thread_local! {
    static THREAD: Cell<Option<Lang>> = const { Cell::new(None) };
}

/// Sprache der laufenden Anfrage.
pub fn current() -> Lang {
    THREAD.with(|c| c.get()).or_else(|| TASK.try_with(|l| *l).ok()).unwrap_or_else(default_lang)
}

pub fn is_en() -> bool {
    current() == Lang::En
}

/// Festen Text in der aktuellen Sprache wählen.
pub fn t(de: &'static str, en: &'static str) -> &'static str {
    if is_en() { en } else { de }
}

/// Asynchronen Ablauf (eine Anfrage) in einer Sprache ausführen.
pub async fn scope<F: std::future::Future>(lang: Lang, f: F) -> F::Output {
    TASK.scope(lang, f).await
}

/// Synchronen Ablauf (z. B. in `spawn_blocking`) in einer Sprache ausführen.
pub fn with<T>(lang: Lang, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Lang>);
    impl Drop for Restore {
        fn drop(&mut self) {
            THREAD.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(THREAD.with(|c| c.replace(Some(lang))));
    f()
}

/// Text mit Platzhaltern in der aktuellen Sprache: `tr!("{n} Seiten", "{n} pages")`.
#[macro_export]
macro_rules! tr {
    ($de:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::is_en() { format!($en $(, $arg)*) } else { format!($de $(, $arg)*) }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_language() {
        assert_eq!(with(Lang::En, || t("Ja", "Yes")), "Yes");
        let n = 3;
        assert_eq!(with(Lang::En, || tr!("{n} Seiten", "{n} pages")), "3 pages");
        assert_eq!(with(Lang::De, || tr!("{n} Seiten", "{n} pages")), "3 Seiten");
        // nach `with` wieder wie vorher
        assert_eq!(THREAD.with(|c| c.get()), None);
    }

    #[test]
    fn cookie() {
        let mut h = axum::http::HeaderMap::new();
        h.insert(axum::http::header::COOKIE, "mimi=abc; mimi-lang=en".parse().unwrap());
        assert_eq!(from_headers(&h), Lang::En);
        h.insert(axum::http::header::COOKIE, "mimi-lang=xx".parse().unwrap());
        assert_eq!(from_headers(&h), default_lang());
    }
}
