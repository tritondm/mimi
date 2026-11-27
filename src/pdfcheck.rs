// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 Mimi contributors

//! Prüfung des erzeugten PDFs.
//!
//! 1. Eigene Basisprüfung (immer): die wichtigsten maschinell prüfbaren
//!    PDF/UA-Punkte, die PAC ebenfalls prüft.
//! 2. veraPDF (falls installiert): vollständige Prüfung nach PDF/UA-1
//!    (Matterhorn-Protokoll), dieselbe Grundlage wie PAC.

use crate::{i18n, tr};
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
    formulas: usize,
    formulas_no_alt: usize,
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
                b"Formula" => {
                    stats.formulas += 1;
                    let has_alt = get(doc, d, b"Alt").map(|a| !text_string(a).trim().is_empty()).unwrap_or(false)
                        || get(doc, d, b"ActualText").is_some();
                    if !has_alt {
                        stats.formulas_no_alt += 1;
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
    let doc = Document::load(path).context(i18n::t("PDF konnte nicht gelesen werden", "The PDF could not be read"))?;
    let pages = doc.get_pages().len();
    let mut checks = Vec::new();
    let catalog = doc.catalog().context(i18n::t("PDF ohne Katalog", "PDF without catalog"))?;

    // getaggt?
    let marked = get(&doc, catalog, b"MarkInfo")
        .and_then(|m| if let Object::Dictionary(d) = m { get(&doc, d, b"Marked").cloned() } else { None })
        .map(|m| matches!(m, Object::Boolean(true)))
        .unwrap_or(false);
    let tree = get(&doc, catalog, b"StructTreeRoot").and_then(|o| if let Object::Dictionary(d) = o { Some(d) } else { None });
    if marked && tree.is_some() {
        checks.push(ok(i18n::t("PDF ist getaggt", "PDF is tagged"), i18n::t("Strukturbaum vorhanden", "structure tree present")));
    } else {
        checks.push(bad("fehler", i18n::t("PDF ist nicht getaggt", "PDF is not tagged"), i18n::t("Beim Export „PDF/UA“ bzw. „Getaggtes PDF“ aktivieren.", "Enable “PDF/UA” or “Tagged PDF” when exporting.")));
    }

    // Sprache
    match get(&doc, catalog, b"Lang").map(text_string) {
        Some(l) if !l.trim().is_empty() => checks.push(ok(i18n::t("Dokumentsprache", "Document language"), l)),
        _ => checks.push(bad("fehler", i18n::t("Dokumentsprache fehlt", "Document language missing"), i18n::t("Im Word-Dokument eine Standardsprache festlegen.", "Set a default language in the Word document."))),
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
        checks.push(bad("fehler", i18n::t("Dokumenttitel fehlt", "Document title missing"), i18n::t("Titel im Word-Dokument unter Eigenschaften setzen.", "Set the title in the Word document under Properties.")));
    } else {
        checks.push(ok(i18n::t("Dokumenttitel", "Document title"), title.trim().to_string()));
    }
    if display_title {
        checks.push(ok(i18n::t("Fenster zeigt den Titel", "Window shows the title"), i18n::t("DisplayDocTitle ist gesetzt", "DisplayDocTitle is set")));
    } else {
        checks.push(bad("fehler", i18n::t("Fenster zeigt Dateinamen statt Titel", "Window shows the file name instead of the title"), i18n::t("DisplayDocTitle fehlt – beim LibreOffice-Export mit PDF/UA wird das automatisch gesetzt.", "DisplayDocTitle is missing – LibreOffice sets it automatically when exporting with PDF/UA.")));
    }
    match &xmp {
        Some(x) if x.contains("pdfuaid") => checks.push(ok(i18n::t("PDF/UA-Kennung", "PDF/UA identifier"), i18n::t("in den Metadaten vorhanden", "present in the metadata"))),
        Some(_) => checks.push(bad("fehler", i18n::t("PDF/UA-Kennung fehlt", "PDF/UA identifier missing"), i18n::t("Metadaten enthalten keine pdfuaid-Angabe.", "The metadata contain no pdfuaid entry."))),
        None => checks.push(bad("fehler", i18n::t("XMP-Metadaten fehlen", "XMP metadata missing"), i18n::t("Ohne Metadaten keine PDF/UA-Kennung.", "Without metadata there is no PDF/UA identifier."))),
    }

    // Strukturbaum
    if let Some(t) = tree {
        let role_map = get(&doc, t, b"RoleMap").and_then(|o| if let Object::Dictionary(d) = o { Some(d) } else { None });
        let remapped: Vec<String> = role_map
            .map(|rm| rm.iter().filter(|(k, _)| STD_TYPES.contains(&k.as_slice())).map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect())
            .unwrap_or_default();
        if remapped.is_empty() {
            checks.push(ok(i18n::t("Standard-Tags", "Standard tags"), i18n::t("keine Standard-Tags umgeleitet", "no standard tags remapped")));
        } else {
            checks.push(bad("fehler", i18n::t("Standard-Tags umgeleitet", "Standard tags remapped"), tr!("Formatvorlage(n) mit reserviertem Namen: {} – Mimi benennt diese beim Export automatisch um; tritt der Fehler trotzdem auf, Vorlage in Word umbenennen.", "Style(s) with a reserved name: {} – Mimi renames them automatically when exporting; if the error still occurs, rename the style in Word.", remapped.join(", "))));
        }
        let mut stats = StructStats::default();
        let mut seen = HashSet::new();
        if let Ok(k) = t.get(b"K") {
            walk(&doc, k, role_map, &mut stats, &mut seen, &mut None, false, 0);
        }
        if stats.figures_no_alt == 0 {
            checks.push(ok(i18n::t("Alternativtexte", "Alternative texts"), tr!("alle {} Abbildungen haben einen Alternativtext", "all {} figures have an alternative text", stats.figures)));
        } else {
            checks.push(bad("fehler", i18n::t("Abbildungen ohne Alternativtext", "Figures without alternative text"), tr!("{} von {} Abbildungen", "{} of {} figures", stats.figures_no_alt, stats.figures)));
        }
        if stats.formulas > 0 {
            if stats.formulas_no_alt == 0 {
                checks.push(ok(i18n::t("Formeln", "Equations"), tr!("alle {} Formeln haben einen Alternativtext", "all {} equations have an alternative text", stats.formulas)));
            } else {
                checks.push(bad("fehler", i18n::t("Formeln ohne Alternativtext", "Equations without alternative text"), tr!("{} von {} Formeln (veraPDF 7.7-1)", "{} of {} equations (veraPDF 7.7-1)", stats.formulas_no_alt, stats.formulas)));
            }
        }
        if stats.headings > 0 {
            checks.push(ok(i18n::t("Überschriften", "Headings"), tr!("{} Überschriften im Strukturbaum", "{} headings in the structure tree", stats.headings)));
        } else {
            checks.push(bad("warnung", i18n::t("Keine Überschriften", "No headings"), i18n::t("Das PDF hat keine Überschriften-Tags.", "The PDF has no heading tags.")));
        }
        if stats.heading_jumps > 0 {
            checks.push(bad("fehler", i18n::t("Überschriftenebene übersprungen", "Heading level skipped"), tr!("{} Mal folgt z. B. H4 direkt auf H2 – im Reiter „Struktur & Hinweise“ korrigierbar.", "{} time(s) e.g. H4 directly follows H2 – can be fixed in the “Structure & hints” tab.", stats.heading_jumps)));
        }
        if stats.nested_blocks == 0 {
            checks.push(ok(i18n::t("Verschachtelung", "Nesting"), i18n::t("keine Bereiche, Absätze oder Überschriften innerhalb von Absätzen", "no areas, paragraphs or headings inside paragraphs")));
        } else {
            checks.push(bad("fehler", i18n::t("Falsch verschachtelte Struktur", "Wrongly nested structure"), tr!("{} Bereich(e), Absätze oder Überschriften stecken in einem Absatz (meist Rahmen oder Textfelder). PAC und PDF/UA-2 bemängeln das.", "{} area(s), paragraphs or headings are inside a paragraph (usually frames or text boxes). PAC and PDF/UA-2 report this.", stats.nested_blocks)));
        }
        if stats.tables_no_th == 0 {
            checks.push(ok(i18n::t("Tabellenköpfe", "Table headers"), tr!("alle {} Tabellen haben Kopfzellen", "all {} tables have header cells", stats.tables)));
        } else {
            checks.push(bad("fehler", i18n::t("Tabellen ohne Kopfzellen", "Tables without header cells"), tr!("{} von {} Tabellen", "{} of {} tables", stats.tables_no_th, stats.tables)));
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
        checks.push(ok(i18n::t("Schriften eingebettet", "Fonts embedded"), i18n::t("alle Schriften sind eingebettet", "all fonts are embedded")));
    } else {
        let mut v: Vec<_> = not_embedded.into_iter().collect();
        v.sort();
        checks.push(bad("fehler", i18n::t("Nicht eingebettete Schriften", "Fonts not embedded"), v.join(", ")));
    }
    if links_no_contents == 0 {
        checks.push(ok(i18n::t("Links beschrieben", "Links described"), tr!("{} Links", "{} links", links_total)));
    } else {
        checks.push(bad("warnung", i18n::t("Links ohne Beschreibung", "Links without description"), tr!("{} von {} Links", "{} of {} links", links_no_contents, links_total)));
    }

    if links_untagged > 0 {
        checks.push(bad("fehler", i18n::t("Links ohne Tag", "Links without tag"), tr!("{} Links sind nicht im Strukturbaum (meist Felder/Links in Kopf- oder Fußzeile).", "{} links are not in the structure tree (usually fields/links in a header or footer).", links_untagged)));
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
            message: i18n::t("veraPDF ist nicht installiert – es wurde nur die Basisprüfung durchgeführt. Siehe README.", "veraPDF is not installed – only the basic check was run. See README.").into(),
            ..Default::default()
        };
    };
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        tokio::process::Command::new(&bin).args(["--flavour", "ua1", "--format", "mrr"]).arg(pdf).kill_on_drop(true).output(),
    )
    .await;
    let out = match out {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return VeraResult { available: true, message: tr!("veraPDF konnte nicht gestartet werden: {e}", "veraPDF could not be started: {e}"), ..Default::default() },
        Err(_) => return VeraResult { available: true, message: i18n::t("veraPDF hat zu lange gebraucht (Zeitüberschreitung).", "veraPDF took too long (timeout).").into(), ..Default::default() },
    };
    match parse_mrr(&out.stdout) {
        Ok(mut r) => {
            r.available = true;
            if r.compliant.is_none() {
                r.message = tr!("veraPDF-Ausgabe konnte nicht ausgewertet werden: {}", "veraPDF output could not be evaluated: {}", String::from_utf8_lossy(&out.stderr).chars().take(500).collect::<String>());
            }
            r
        }
        Err(e) => VeraResult { available: true, message: tr!("veraPDF-Ausgabe unlesbar: {e}", "veraPDF output unreadable: {e}"), ..Default::default() },
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
