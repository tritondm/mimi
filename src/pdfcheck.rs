// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 example.org

//! Prüfung des erzeugten PDFs.
//!
//! 1. Eigene Basisprüfung (immer): die wichtigsten maschinell prüfbaren
//!    PDF/UA-Punkte, die PAC ebenfalls prüft.
//! 2. veraPDF (falls installiert): vollständige Prüfung nach PDF/UA-1
//!    (Matterhorn-Protokoll), dieselbe Grundlage wie PAC.

use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object};
use quick_xml::events::Event;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone)]
pub struct Check {
    pub ok: bool,
    /// "fehler" oder "warnung" (nur relevant wenn !ok)
    pub severity: String,
    pub title: String,
    pub detail: String,
}

#[derive(Serialize, Default)]
pub struct VeraResult {
    pub available: bool,
    pub compliant: Option<bool>,
    pub failed_rules: Vec<VeraRule>,
    pub message: String,
}

#[derive(Serialize, Clone)]
pub struct VeraRule {
    pub clause: String,
    pub test: String,
    pub failed_checks: String,
    pub description: String,
}

#[derive(Serialize)]
pub struct PdfReport {
    pub pages: usize,
    pub checks: Vec<Check>,
    pub verapdf: VeraResult,
}

fn ok(title: &str, detail: impl Into<String>) -> Check {
    Check { ok: true, severity: String::new(), title: title.into(), detail: detail.into() }
}
fn bad(sev: &str, title: &str, detail: impl Into<String>) -> Check {
    Check { ok: false, severity: sev.into(), title: title.into(), detail: detail.into() }
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    match o {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(o),
        _ => o,
    }
}

fn dict<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    match deref(doc, o) {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

fn get<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key).ok().map(|o| deref(doc, o))
}

pub(crate) fn text_string(o: &Object) -> String {
    match o {
        Object::String(b, _) => {
            if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
                let u: Vec<u16> = b[2..].chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                String::from_utf16_lossy(&u)
            } else if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
                String::from_utf8_lossy(&b[3..]).into_owned()
            } else {
                b.iter().map(|&c| c as char).collect()
            }
        }
        _ => String::new(),
    }
}

#[derive(Default)]
struct StructStats {
    figures: usize,
    figures_no_alt: usize,
    tables: usize,
    tables_no_th: usize,
    headings: usize,
    elements: usize,
    last_heading: u8,
    heading_jumps: usize,
    /// Blockelemente (Div, P, Hn …) innerhalb eines Absatzes
    nested_blocks: usize,
}

/// Standard-Strukturtypen (ISO 32000-1, 14.8.4) – dürfen laut PDF/UA nicht umgeleitet werden.
const STD_TYPES: &[&[u8]] = &[
    b"Document", b"Part", b"Art", b"Sect", b"Div", b"BlockQuote", b"Caption", b"TOC", b"TOCI", b"Index", b"NonStruct",
    b"Private", b"P", b"H", b"H1", b"H2", b"H3", b"H4", b"H5", b"H6", b"L", b"LI", b"Lbl", b"LBody", b"Table", b"TR",
    b"TH", b"TD", b"THead", b"TBody", b"TFoot", b"Span", b"Quote", b"Note", b"Reference", b"BibEntry", b"Code", b"Link",
    b"Annot", b"Ruby", b"RB", b"RT", b"RP", b"Warichu", b"WT", b"WP", b"Figure", b"Formula", b"Form",
];

#[allow(clippy::too_many_arguments)]
fn walk(doc: &Document, o: &Object, role_map: Option<&Dictionary>, stats: &mut StructStats, seen: &mut HashSet<(u32, u16)>, in_table: &mut Option<bool>, in_para: bool, depth: usize) {
    if depth > 200 {
        return;
    }
    if let Object::Reference(id) = o {
        if !seen.insert(*id) {
            return;
        }
    }
    match deref(doc, o) {
        Object::Array(a) => {
            for k in a {
                walk(doc, k, role_map, stats, seen, in_table, in_para, depth + 1);
            }
        }
        Object::Dictionary(d) => {
            let Some(Object::Name(s)) = get(doc, d, b"S") else {
                return;
            };
            let role = crate::pdffix::resolve_role(role_map, s);
            stats.elements += 1;
            if in_para && crate::pdffix::BLOCK_TYPES.contains(&role.as_slice()) {
                stats.nested_blocks += 1;
            }
            let para = crate::pdffix::PARA_TYPES.contains(&role.as_slice());
            match role.as_slice() {
                b"Figure" => {
                    stats.figures += 1;
                    let has_alt = get(doc, d, b"Alt").map(|a| !text_string(a).trim().is_empty()).unwrap_or(false)
                        || get(doc, d, b"ActualText").is_some();
                    if !has_alt {
                        stats.figures_no_alt += 1;
                    }
                }
                b"H" => stats.headings += 1,
                b"H1" | b"H2" | b"H3" | b"H4" | b"H5" | b"H6" => {
                    stats.headings += 1;
                    let lvl = role[1] - b'0';
                    if stats.last_heading > 0 && lvl > stats.last_heading + 1 {
                        stats.heading_jumps += 1;
                    }
                    stats.last_heading = lvl;
                }
                b"TH" => {
                    if let Some(t) = in_table.as_mut() {
                        *t = true;
                    }
                }
                _ => {}
            }
            let is_table = role.as_slice() == b"Table";
            let mut local = if is_table { Some(false) } else { None };
            if let Some(k) = d.get(b"K").ok() {
                if is_table {
                    walk(doc, k, role_map, stats, seen, &mut local, para, depth + 1);
                } else {
                    walk(doc, k, role_map, stats, seen, in_table, para, depth + 1);
                }
            }
            if is_table {
                stats.tables += 1;
                if local == Some(false) {
                    stats.tables_no_th += 1;
                }
            }
        }
        _ => {}
    }
}

pub fn basic_checks(path: &Path) -> Result<(usize, Vec<Check>)> {
    let doc = Document::load(path).context("PDF konnte nicht gelesen werden")?;
    let pages = doc.get_pages().len();
    let mut checks = Vec::new();
    let catalog = doc.catalog().context("PDF ohne Katalog")?;

    // getaggt?
    let marked = get(&doc, catalog, b"MarkInfo")
        .and_then(|m| if let Object::Dictionary(d) = m { get(&doc, d, b"Marked").cloned() } else { None })
        .map(|m| matches!(m, Object::Boolean(true)))
        .unwrap_or(false);
    let tree = get(&doc, catalog, b"StructTreeRoot").and_then(|o| if let Object::Dictionary(d) = o { Some(d) } else { None });
    if marked && tree.is_some() {
        checks.push(ok("PDF ist getaggt", "Strukturbaum vorhanden"));
    } else {
        checks.push(bad("fehler", "PDF ist nicht getaggt", "Beim Export „PDF/UA“ bzw. „Getaggtes PDF“ aktivieren."));
    }

    // Sprache
    match get(&doc, catalog, b"Lang").map(text_string) {
        Some(l) if !l.trim().is_empty() => checks.push(ok("Dokumentsprache", l)),
        _ => checks.push(bad("fehler", "Dokumentsprache fehlt", "Im Word-Dokument eine Standardsprache festlegen.")),
    }

    // Titel anzeigen
    let display_title = get(&doc, catalog, b"ViewerPreferences")
        .and_then(|v| if let Object::Dictionary(d) = v { get(&doc, d, b"DisplayDocTitle").cloned() } else { None })
        .map(|v| matches!(v, Object::Boolean(true)))
        .unwrap_or(false);

    // XMP-Metadaten
    let xmp = get(&doc, catalog, b"Metadata").and_then(|m| match m {
        Object::Stream(s) => {
            let raw = s.decompressed_content().unwrap_or_else(|_| s.content.clone());
            Some(String::from_utf8_lossy(&raw).into_owned())
        }
        _ => None,
    });
    let mut title = String::new();
    if let Ok(info) = doc.trailer.get(b"Info") {
        if let Some(d) = dict(&doc, info) {
            if let Some(t) = get(&doc, d, b"Title") {
                title = text_string(t);
            }
        }
    }
    if let Some(x) = &xmp {
        if title.trim().is_empty() {
            if let Some(i) = x.find("<dc:title>") {
                let rest = &x[i..];
                if let (Some(a), Some(b)) = (rest.find("<rdf:li"), rest.find("</rdf:li>")) {
                    if let Some(gt) = rest[a..].find('>') {
                        if a + gt + 1 < b {
                            title = rest[a + gt + 1..b].to_string();
                        }
                    }
                }
            }
        }
    }
    if title.trim().is_empty() {
        checks.push(bad("fehler", "Dokumenttitel fehlt", "Titel im Word-Dokument unter Eigenschaften setzen."));
    } else {
        checks.push(ok("Dokumenttitel", title.trim().to_string()));
    }
    if display_title {
        checks.push(ok("Fenster zeigt den Titel", "DisplayDocTitle ist gesetzt"));
    } else {
        checks.push(bad("fehler", "Fenster zeigt Dateinamen statt Titel", "DisplayDocTitle fehlt – beim LibreOffice-Export mit PDF/UA wird das automatisch gesetzt."));
    }
    match &xmp {
        Some(x) if x.contains("pdfuaid") => checks.push(ok("PDF/UA-Kennung", "in den Metadaten vorhanden")),
        Some(_) => checks.push(bad("fehler", "PDF/UA-Kennung fehlt", "Metadaten enthalten keine pdfuaid-Angabe.")),
        None => checks.push(bad("fehler", "XMP-Metadaten fehlen", "Ohne Metadaten keine PDF/UA-Kennung.")),
    }

    // Strukturbaum
    if let Some(t) = tree {
        let role_map = get(&doc, t, b"RoleMap").and_then(|o| if let Object::Dictionary(d) = o { Some(d) } else { None });
        let remapped: Vec<String> = role_map
            .map(|rm| rm.iter().filter(|(k, _)| STD_TYPES.contains(&k.as_slice())).map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect())
            .unwrap_or_default();
        if remapped.is_empty() {
            checks.push(ok("Standard-Tags", "keine Standard-Tags umgeleitet"));
        } else {
            checks.push(bad("fehler", "Standard-Tags umgeleitet", format!("Formatvorlage(n) mit reserviertem Namen: {} – Rampe benennt diese beim Export automatisch um; tritt der Fehler trotzdem auf, Vorlage in Word umbenennen.", remapped.join(", "))));
        }
        let mut stats = StructStats::default();
        let mut seen = HashSet::new();
        if let Ok(k) = t.get(b"K") {
            walk(&doc, k, role_map, &mut stats, &mut seen, &mut None, false, 0);
        }
        if stats.figures_no_alt == 0 {
            checks.push(ok("Alternativtexte", format!("alle {} Abbildungen haben einen Alternativtext", stats.figures)));
        } else {
            checks.push(bad("fehler", "Abbildungen ohne Alternativtext", format!("{} von {} Abbildungen", stats.figures_no_alt, stats.figures)));
        }
        if stats.headings > 0 {
            checks.push(ok("Überschriften", format!("{} Überschriften im Strukturbaum", stats.headings)));
        } else {
            checks.push(bad("warnung", "Keine Überschriften", "Das PDF hat keine Überschriften-Tags."));
        }
        if stats.heading_jumps > 0 {
            checks.push(bad("fehler", "Überschriftenebene übersprungen", format!("{} Mal folgt z. B. H4 direkt auf H2 – im Reiter „Struktur & Hinweise“ korrigierbar.", stats.heading_jumps)));
        }
        if stats.nested_blocks == 0 {
            checks.push(ok("Verschachtelung", "keine Bereiche, Absätze oder Überschriften innerhalb von Absätzen"));
        } else {
            checks.push(bad("fehler", "Falsch verschachtelte Struktur", format!("{} Bereich(e), Absätze oder Überschriften stecken in einem Absatz (meist Rahmen oder Textfelder). PAC und PDF/UA-2 bemängeln das.", stats.nested_blocks)));
        }
        if stats.tables_no_th == 0 {
            checks.push(ok("Tabellenköpfe", format!("alle {} Tabellen haben Kopfzellen", stats.tables)));
        } else {
            checks.push(bad("fehler", "Tabellen ohne Kopfzellen", format!("{} von {} Tabellen", stats.tables_no_th, stats.tables)));
        }
    }

    // Schriften eingebettet
    let mut not_embedded = HashSet::new();
    let mut links_total = 0;
    let mut links_no_contents = 0;
    let mut links_untagged = 0;
    for obj in doc.objects.values() {
        if let Some(d) = match obj {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        } {
            match d.get(b"Type").ok() {
                Some(Object::Name(n)) if n == b"FontDescriptor" => {
                    if !(d.has(b"FontFile") || d.has(b"FontFile2") || d.has(b"FontFile3")) {
                        if let Ok(Object::Name(f)) = d.get(b"FontName") {
                            not_embedded.insert(String::from_utf8_lossy(f).into_owned());
                        }
                    }
                }
                _ => {}
            }
            if let Ok(Object::Name(st)) = d.get(b"Subtype") {
                if st == b"Link" {
                    links_total += 1;
                    if !d.has(b"StructParent") {
                        links_untagged += 1;
                    }
                    let has = d.get(b"Contents").map(|c| !text_string(c).trim().is_empty()).unwrap_or(false);
                    if !has {
                        links_no_contents += 1;
                    }
                }
            }
        }
    }
    if not_embedded.is_empty() {
        checks.push(ok("Schriften eingebettet", "alle Schriften sind eingebettet"));
    } else {
        let mut v: Vec<_> = not_embedded.into_iter().collect();
        v.sort();
        checks.push(bad("fehler", "Nicht eingebettete Schriften", v.join(", ")));
    }
    if links_no_contents == 0 {
        checks.push(ok("Links beschrieben", format!("{} Links", links_total)));
    } else {
        checks.push(bad("warnung", "Links ohne Beschreibung", format!("{} von {} Links", links_no_contents, links_total)));
    }

    if links_untagged > 0 {
        checks.push(bad("fehler", "Links ohne Tag", format!("{} Links sind nicht im Strukturbaum (meist Felder/Links in Kopf- oder Fußzeile).", links_untagged)));
    }

    Ok((pages, checks))
}

// ---------------------------------------------------------------------------
// veraPDF
// ---------------------------------------------------------------------------

pub fn find_verapdf(base: &Path) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VERAPDF") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    for cand in [base.join("tools/verapdf/verapdf"), base.join("tools/verapdf/verapdf.bat")] {
        if cand.exists() {
            return Some(cand);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("verapdf")).find(|p| p.exists())
}

pub async fn run_verapdf(bin: Option<PathBuf>, pdf: &Path) -> VeraResult {
    let Some(bin) = bin else {
        return VeraResult {
            available: false,
            message: "veraPDF ist nicht installiert – es wurde nur die Basisprüfung durchgeführt. Siehe README.".into(),
            ..Default::default()
        };
    };
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        tokio::process::Command::new(&bin).args(["--flavour", "ua1", "--format", "mrr"]).arg(pdf).output(),
    )
    .await;
    let out = match out {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return VeraResult { available: true, message: format!("veraPDF konnte nicht gestartet werden: {e}"), ..Default::default() },
        Err(_) => return VeraResult { available: true, message: "veraPDF hat zu lange gebraucht (Zeitüberschreitung).".into(), ..Default::default() },
    };
    match parse_mrr(&out.stdout) {
        Ok(mut r) => {
            r.available = true;
            if r.compliant.is_none() {
                r.message = format!("veraPDF-Ausgabe konnte nicht ausgewertet werden: {}", String::from_utf8_lossy(&out.stderr).chars().take(500).collect::<String>());
            }
            r
        }
        Err(e) => VeraResult { available: true, message: format!("veraPDF-Ausgabe unlesbar: {e}"), ..Default::default() },
    }
}

fn parse_mrr(xml: &[u8]) -> Result<VeraResult> {
    let mut r = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut res = VeraResult::default();
    let mut cur: Option<VeraRule> = None;
    let mut in_desc = false;
    let a = |e: &quick_xml::events::BytesStart, k: &[u8]| {
        e.attributes().flatten().find(|x| x.key.as_ref() == k).and_then(|x| x.unescape_value().ok().map(|v| v.into_owned())).unwrap_or_default()
    };
    loop {
        match r.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                b"validationReport" => res.compliant = Some(a(&e, b"isCompliant") == "true"),
                b"rule" if a(&e, b"status") == "failed" => {
                    cur = Some(VeraRule { clause: a(&e, b"clause"), test: a(&e, b"testNumber"), failed_checks: a(&e, b"failedChecks"), description: String::new() });
                }
                b"description" if cur.is_some() => in_desc = true,
                _ => {}
            },
            Event::Text(t) if in_desc => {
                if let Some(c) = cur.as_mut() {
                    c.description.push_str(&t.unescape()?);
                }
            }
            Event::End(e) => match e.name().as_ref() {
                b"description" => in_desc = false,
                b"rule" => {
                    if let Some(c) = cur.take() {
                        res.failed_rules.push(c);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        buf.clear();
    }
    Ok(res)
}
