// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 Andreas Kraxner

//! Analyse und Korrektur von Word-Dateien (DOCX) im Hinblick auf Barrierefreiheit.
//!
//! Die XML-Teile werden ereignisbasiert gelesen und unverändert durchgereicht;
//! nur die gezielt geänderten Elemente werden neu geschrieben. Dadurch bleibt
//! der Rest des Dokuments Byte für Byte erhalten.

use crate::{i18n, tr};
use anyhow::{Context, Result};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Cursor, Write};

const DECORATIVE_URI: &str = "{C183D7F6-B498-43B3-948B-1728B52AA6E4}";
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_ADEC: &str = "http://schemas.microsoft.com/office/drawing/2017/decorative";

/// Alternativtext, mit dem die Export-Kopie dekorative Bilder und Formen kennzeichnet.
/// LibreOffice beachtet „dekorativ“ nur bei Bildern; `pdffix` macht Abbildungen mit
/// diesem Alternativtext nach dem Export zu Artefakten.
pub const DECORATIVE_ALT: &str = "mimi:dekorativ";

// ---------------------------------------------------------------------------
// Ergebnis-Strukturen
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Default)]
pub struct ImageInfo {
    pub index: usize,
    pub id: String,
    pub name: String,
    pub alt: String,
    pub title: String,
    pub decorative: bool,
    /// Pfad innerhalb von word/, z. B. "media/image3.png"
    pub media: Option<String>,
    /// "bild", "diagramm", "form"
    pub kind: String,
    pub caption: String,
    pub section: String,
    /// Textstellen in einer EMF/WMF-Vektorgrafik (0 = keine oder kein Vektorformat)
    pub vector_text: usize,
    #[serde(skip)]
    para: usize,
}

#[derive(Serialize, Clone)]
pub struct TableInfo {
    pub index: usize,
    pub header: bool,
    pub rows: usize,
    pub first_row: String,
    pub nested: bool,
    pub section: String,
}

#[derive(Serialize, Clone)]
pub struct HeadingInfo {
    pub level: u8,
    pub text: String,
    /// Absatznummer im Dokument (für Korrekturen)
    pub para: usize,
    pub style: String,
}

/// Übersprungene Überschriftsebene, die das Tool korrigieren kann.
#[derive(Serialize, Clone)]
pub struct HeadingFix {
    pub para: usize,
    pub text: String,
    pub from: u8,
    pub to: u8,
    /// Formatvorlage (ID), die die richtige Ebene hat
    pub style: String,
    pub style_name: String,
    pub section: String,
}

/// Schriftfarbe mit zu wenig Kontrast zu weißem Hintergrund (WCAG 1.4.3).
#[derive(Serialize, Clone)]
pub struct ColorIssue {
    /// z. B. "E6320F"
    pub color: String,
    pub ratio: f64,
    /// Vorschlag: gleicher Farbton, so weit abgedunkelt, dass es reicht
    pub suggestion: String,
    pub suggestion_ratio: f64,
    /// wo die Farbe vorkommt (Formatvorlagen / direkte Formatierung)
    pub places: Vec<String>,
}

#[derive(Serialize, Clone, Default)]
pub struct Hint {
    /// "fehler", "warnung", "hinweis"
    pub severity: String,
    pub kind: String,
    pub text: String,
    pub section: String,
    /// Absatz der Überschrift bei übersprungenen Ebenen (passt zu `HeadingFix::para`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub para: Option<usize>,
}

#[derive(Serialize)]
pub struct Analysis {
    pub title: String,
    pub language: String,
    pub images: Vec<ImageInfo>,
    pub tables: Vec<TableInfo>,
    pub headings: Vec<HeadingInfo>,
    pub hints: Vec<Hint>,
    /// Links insgesamt / Links ohne Beschreibung (QuickInfo)
    pub links_total: usize,
    pub links_missing: usize,
    pub heading_fixes: Vec<HeadingFix>,
    pub colors: Vec<ColorIssue>,
    /// Was beim PDF-Export automatisch vorbereitet wird
    pub pdf_prep: Vec<String>,
    /// Formeln aus dem Formeleditor (brauchen im PDF einen Alternativtext)
    pub formulas: Vec<crate::formula::FormulaInfo>,
}

#[derive(Deserialize, Default)]
pub struct Changes {
    /// Überschriften auf eine andere Formatvorlage setzen
    #[serde(default)]
    pub headings: Vec<HeadingChange>,
    /// Schriftfarben ersetzen
    #[serde(default)]
    pub colors: Vec<ColorChange>,
    pub title: Option<String>,
    pub language: Option<String>,
    #[serde(default)]
    pub images: Vec<ImageChange>,
    #[serde(default)]
    pub tables: Vec<TableChange>,
    /// Links ohne QuickInfo bekommen ihren sichtbaren Text als QuickInfo.
    /// LibreOffice übernimmt diese ins PDF als Linkbeschreibung (/Contents).
    #[serde(default)]
    pub link_descriptions: bool,
    /// eigene Alternativtexte für Formeln (leer = Linearform)
    #[serde(default)]
    pub formulas: Vec<crate::formula::FormulaChange>,
}

#[derive(Deserialize)]
pub struct ImageChange {
    pub index: usize,
    pub alt: String,
    pub decorative: bool,
}

#[derive(Deserialize)]
pub struct TableChange {
    pub index: usize,
    pub header: bool,
}

#[derive(Deserialize)]
pub struct HeadingChange {
    pub para: usize,
    pub style: String,
}

#[derive(Deserialize)]
pub struct ColorChange {
    pub from: String,
    pub to: String,
}

// ---------------------------------------------------------------------------
// Kontrast (WCAG 2.x)
// ---------------------------------------------------------------------------

fn parse_hex(h: &str) -> Option<[f64; 3]> {
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f64 / 255.0);
    Some([c(0)?, c(2)?, c(4)?])
}

fn luminance(rgb: [f64; 3]) -> f64 {
    let lin = |x: f64| if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

/// Kontrast gegen Weiß
fn contrast_white(rgb: [f64; 3]) -> f64 {
    1.05 / (luminance(rgb) + 0.05)
}

/// Gleicher Farbton, abgedunkelt bis der Kontrast gegen Weiß mindestens `target` ist.
fn darken_to(rgb: [f64; 3], target: f64) -> [u8; 3] {
    let to8 = |f: f64| -> [u8; 3] { [(rgb[0] * f * 255.0).round() as u8, (rgb[1] * f * 255.0).round() as u8, (rgb[2] * f * 255.0).round() as u8] };
    let ratio8 = |c: [u8; 3]| contrast_white([c[0] as f64 / 255.0, c[1] as f64 / 255.0, c[2] as f64 / 255.0]);
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if ratio8(to8(mid)) >= target { lo = mid } else { hi = mid }
    }
    to8(lo)
}

const MIN_CONTRAST: f64 = 4.5;
/// Vorschläge mit etwas Reserve, damit Rundungen in PDF/PAC nicht kippen
const TARGET_CONTRAST: f64 = 4.6;

/// Standard-Strukturtypen aus ISO 32000-1, 14.8.4. Eine eigene Formatvorlage mit
/// genau diesem Namen würde LibreOffice im PDF per RoleMap auf P umleiten –
/// das verbietet PDF/UA (7.1) und macht z. B. alle Tabellenzellen (TD) kaputt.
const PDF_STD_TYPES: &[&str] = &[
    "Document", "Part", "Art", "Sect", "Div", "BlockQuote", "Caption", "TOC", "TOCI", "Index", "NonStruct", "Private",
    "P", "H", "H1", "H2", "H3", "H4", "H5", "H6", "L", "LI", "Lbl", "LBody", "Table", "TR", "TH", "TD", "THead",
    "TBody", "TFoot", "Span", "Quote", "Note", "Reference", "BibEntry", "Code", "Link", "Annot", "Ruby", "RB", "RT",
    "RP", "Warichu", "WT", "WP", "Figure", "Formula", "Form", "Artifact",
];

/// Feldtypen, die LibreOffice im PDF als Sprungmarke (Link) ausgibt.
const LINK_FIELDS: &[&str] = &["STYLEREF", "REF", "PAGEREF", "NOTEREF", "HYPERLINK"];

fn field_is_link(instr: &str) -> bool {
    let first = instr.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
    LINK_FIELDS.contains(&first.as_str())
}

// ---------------------------------------------------------------------------
// Hilfsfunktionen
// ---------------------------------------------------------------------------

fn attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

/// w:b, w:tblHeader usw.: fehlendes w:val oder alles außer 0/false bedeutet "an".
fn on_off(e: &BytesStart) -> bool {
    !matches!(attr(e, b"w:val").as_deref(), Some("0") | Some("false") | Some("off"))
}

fn reader(xml: &[u8]) -> Reader<&[u8]> {
    let mut r = Reader::from_reader(xml);
    r.config_mut().trim_text(false);
    r
}

pub fn read_entry(zip_bytes: &[u8], name: &str) -> Result<Option<Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(zip_bytes)).context(i18n::t("Keine gültige DOCX-Datei", "Not a valid DOCX file"))?;
    let result = match zip.by_name(name) {
        Ok(f) => Some(crate::sanitize::read_limited(f)?),
        Err(_) => None,
    };
    Ok(result)
}

fn snippet(s: &str, n: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= n {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(n).collect::<String>())
    }
}

// ---------------------------------------------------------------------------
// Formatvorlagen
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct StyleInfo {
    name: String,
    based_on: Option<String>,
    outline: Option<u8>,
    bold: bool,
}

struct Styles {
    map: HashMap<String, StyleInfo>,
    default_para: Option<String>,
    language: String,
}

impl Styles {
    fn parse(xml: &[u8]) -> Result<Styles> {
        let mut r = reader(xml);
        let mut buf = Vec::new();
        let mut map = HashMap::new();
        let mut default_para = None;
        let mut language = String::new();
        let mut cur: Option<(String, StyleInfo)> = None;
        let mut in_rpr_default = false;
        let mut in_rpr = false;
        loop {
            match r.read_event_into(&mut buf)? {
                Event::Eof => break,
                Event::Start(e) | Event::Empty(e) => {
                    let n = e.name();
                    match n.as_ref() {
                        b"w:style" => {
                            if attr(&e, b"w:type").as_deref() == Some("paragraph") {
                                let id = attr(&e, b"w:styleId").unwrap_or_default();
                                if attr(&e, b"w:default").as_deref() == Some("1") {
                                    default_para = Some(id.clone());
                                }
                                cur = Some((id, StyleInfo::default()));
                            }
                        }
                        b"w:rPrDefault" => in_rpr_default = true,
                        b"w:rPr" => in_rpr = true,
                        b"w:lang" if in_rpr_default => {
                            language = attr(&e, b"w:val").unwrap_or_default();
                        }
                        b"w:name" => {
                            if let Some((_, s)) = cur.as_mut() {
                                s.name = attr(&e, b"w:val").unwrap_or_default();
                            }
                        }
                        b"w:basedOn" => {
                            if let Some((_, s)) = cur.as_mut() {
                                s.based_on = attr(&e, b"w:val");
                            }
                        }
                        b"w:outlineLvl" => {
                            if let Some((_, s)) = cur.as_mut() {
                                s.outline = attr(&e, b"w:val").and_then(|v| v.parse().ok());
                            }
                        }
                        b"w:b" if in_rpr => {
                            if let Some((_, s)) = cur.as_mut() {
                                s.bold = on_off(&e);
                            }
                        }
                        _ => {}
                    }
                }
                Event::End(e) => match e.name().as_ref() {
                    b"w:style" => {
                        if let Some((id, s)) = cur.take() {
                            map.insert(id, s);
                        }
                    }
                    b"w:rPrDefault" => in_rpr_default = false,
                    b"w:rPr" => in_rpr = false,
                    _ => {}
                },
                _ => {}
            }
            buf.clear();
        }
        Ok(Styles { map, default_para, language })
    }

    fn chain<'a>(&'a self, id: &str) -> Vec<&'a StyleInfo> {
        let mut out = Vec::new();
        let mut cur = Some(id.to_string());
        while let Some(c) = cur {
            if out.len() > 15 {
                break;
            }
            match self.map.get(&c) {
                Some(s) => {
                    out.push(s);
                    cur = s.based_on.clone();
                }
                None => break,
            }
        }
        out
    }

    /// Überschriftsebene 1..9, falls die Vorlage (oder eine Basis) eine Überschrift ist.
    fn heading_level(&self, id: &str) -> Option<u8> {
        for s in self.chain(id) {
            if let Some(l) = s.outline {
                return if l < 9 { Some(l + 1) } else { None };
            }
            let n = s.name.to_lowercase();
            if let Some(rest) = n.strip_prefix("heading ") {
                if let Ok(k) = rest.trim().parse::<u8>() {
                    return Some(k);
                }
            }
        }
        None
    }

    fn is_caption(&self, id: &str) -> bool {
        self.chain(id).iter().any(|s| s.name.eq_ignore_ascii_case("caption"))
    }

    fn is_bold(&self, id: &str) -> bool {
        self.chain(id).first().map(|s| s.bold).unwrap_or(false)
    }

    fn is_toc_or_index(&self, id: &str) -> bool {
        self.chain(id).iter().any(|s| {
            let n = s.name.to_lowercase();
            n.starts_with("toc") || n.starts_with("table of figures") || n.starts_with("index")
        })
    }
}

// ---------------------------------------------------------------------------
// Beziehungen (r:embed -> media/…)
// ---------------------------------------------------------------------------

fn parse_rels(xml: &[u8]) -> Result<HashMap<String, String>> {
    let mut r = reader(xml);
    let mut buf = Vec::new();
    let mut map = HashMap::new();
    loop {
        match r.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"Relationship" => {
                if let (Some(id), Some(t)) = (attr(&e, b"Id"), attr(&e, b"Target")) {
                    map.insert(id, t.trim_start_matches("/word/").to_string());
                }
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(map)
}

fn core_title(xml: &[u8]) -> Result<String> {
    let mut r = reader(xml);
    let mut buf = Vec::new();
    let mut inside = false;
    let mut title = String::new();
    loop {
        match r.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) if e.name().as_ref() == b"dc:title" => inside = true,
            Event::End(e) if e.name().as_ref() == b"dc:title" => inside = false,
            Event::Text(t) if inside => title.push_str(&t.unescape()?),
            _ => {}
        }
        buf.clear();
    }
    Ok(title.trim().to_string())
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------

/// Für jeden w:hyperlink in Dokumentreihenfolge: (hat QuickInfo, bereinigter sichtbarer Text)
fn hyperlink_texts(xml: &[u8]) -> Result<Vec<(bool, String)>> {
    let mut r = reader(xml);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut cur: Option<(bool, String)> = None;
    let mut in_t = false;
    loop {
        match r.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) if e.name().as_ref() == b"w:hyperlink" => {
                let has = attr(&e, b"w:tooltip").map(|t| !t.trim().is_empty()).unwrap_or(false);
                cur = Some((has, String::new()));
            }
            Event::Empty(e) if e.name().as_ref() == b"w:hyperlink" => {
                out.push((attr(&e, b"w:tooltip").is_some(), String::new()));
            }
            Event::Start(e) if e.name().as_ref() == b"w:t" => in_t = true,
            Event::End(e) if e.name().as_ref() == b"w:t" => in_t = false,
            Event::Start(e) | Event::Empty(e) if matches!(e.name().as_ref(), b"w:tab" | b"w:br") => {
                if let Some(c) = cur.as_mut() {
                    c.1.push(' ');
                }
            }
            Event::Text(t) if in_t => {
                if let Some(c) = cur.as_mut() {
                    c.1.push_str(&t.unescape()?);
                }
            }
            Event::End(e) if e.name().as_ref() == b"w:hyperlink" => {
                if let Some((has, text)) = cur.take() {
                    out.push((has, clean_link_text(&text)));
                }
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// Leerraum zusammenfassen und eine Seitenzahl am Ende (Verzeichniseinträge) entfernen.
fn clean_link_text(s: &str) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    let mut words = words.as_slice();
    if words.len() > 1 && words.last().map(|w| w.chars().all(|c| c.is_ascii_digit())).unwrap_or(false) {
        words = &words[..words.len() - 1];
    }
    words.join(" ")
}

// ---------------------------------------------------------------------------
// Analyse
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Para {
    style: String,
    text: String,
    outline: Option<u8>,
    has_num: bool,
    has_drawing: bool,
    has_break: bool,
    text_runs: usize,
    bold_runs: usize,
    in_table: bool,
}

struct TableState {
    index: usize,
    rows: usize,
    first_row_text: String,
    header: bool,
}

struct ParaRecord {
    style: String,
    text: String,
}

pub fn analyze(docx: &[u8]) -> Result<Analysis> {
    let doc = read_entry(docx, "word/document.xml")?.context(i18n::t("word/document.xml fehlt – ist das eine Word-Datei?", "word/document.xml is missing – is this a Word file?"))?;
    let styles = match read_entry(docx, "word/styles.xml")? {
        Some(s) => Styles::parse(&s)?,
        None => Styles { map: HashMap::new(), default_para: None, language: String::new() },
    };
    let rels = match read_entry(docx, "word/_rels/document.xml.rels")? {
        Some(r) => parse_rels(&r)?,
        None => HashMap::new(),
    };
    let title = match read_entry(docx, "docProps/core.xml")? {
        Some(c) => core_title(&c)?,
        None => String::new(),
    };

    let mut r = reader(&doc);
    let mut buf = Vec::new();

    let mut paras: Vec<ParaRecord> = Vec::new();
    let mut para_stack: Vec<(usize, Para)> = Vec::new();
    let mut tables: Vec<TableInfo> = Vec::new();
    let mut table_stack: Vec<TableState> = Vec::new();
    let mut table_count = 0usize;
    let mut row_depth_first: Vec<bool> = Vec::new(); // pro offener Zeile: ist erste Zeile?
    let mut in_trpr = false;

    let mut images: Vec<ImageInfo> = Vec::new();
    let mut drawing_stack: Vec<Option<usize>> = Vec::new();
    let mut in_docpr = false;

    let mut headings: Vec<HeadingInfo> = Vec::new();
    let mut hints: Vec<Hint> = Vec::new();
    let mut section = String::new();
    let mut last_level: u8 = 0;
    // (Absatz, Ebene, vorherige Ebene, Text, Abschnitt)
    let mut jumps: Vec<(usize, u8, u8, String, String)> = Vec::new();

    let mut in_t = false;
    let mut in_rpr = false;
    let mut run_bold: Option<bool> = None;
    let mut run_has_text = false;
    let mut in_hyperlink: Option<String> = None;
    let mut url_links = 0usize;

    let mut empty_streak = 0usize;
    let mut empty_after = String::new();

    loop {
        let ev = r.read_event_into(&mut buf)?;
        match &ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let is_empty = matches!(ev, Event::Empty(_));
                match e.name().as_ref() {
                    b"w:p" => {
                        let idx = paras.len();
                        paras.push(ParaRecord { style: String::new(), text: String::new() });
                        let p = Para { in_table: !table_stack.is_empty(), ..Default::default() };
                        if is_empty {
                            // leerer Absatz ohne Inhalt
                            empty_streak += 1;
                        } else {
                            para_stack.push((idx, p));
                        }
                    }
                    b"w:pStyle" => {
                        if let Some((_, p)) = para_stack.last_mut() {
                            p.style = attr(e, b"w:val").unwrap_or_default();
                        }
                    }
                    b"w:outlineLvl" => {
                        if let Some((_, p)) = para_stack.last_mut() {
                            p.outline = attr(e, b"w:val").and_then(|v| v.parse().ok());
                        }
                    }
                    b"w:numPr" => {
                        if let Some((_, p)) = para_stack.last_mut() {
                            p.has_num = true;
                        }
                    }
                    b"w:br" | b"w:sectPr" => {
                        if let Some((_, p)) = para_stack.last_mut() {
                            p.has_break = true;
                        }
                    }
                    b"w:r" if !is_empty => {
                        run_bold = None;
                        run_has_text = false;
                    }
                    b"w:rPr" if !is_empty => in_rpr = true,
                    b"w:b" if in_rpr => run_bold = Some(on_off(e)),
                    b"w:t" if !is_empty => in_t = true,
                    b"w:tab" => {
                        if let Some((_, p)) = para_stack.last_mut() {
                            p.text.push(' ');
                        }
                    }
                    b"w:hyperlink" if !is_empty => in_hyperlink = Some(String::new()),
                    b"w:tbl" if !is_empty => {
                        table_stack.push(TableState { index: table_count, rows: 0, first_row_text: String::new(), header: false });
                        table_count += 1;
                        tables.push(TableInfo { index: tables.len(), header: false, rows: 0, first_row: String::new(), nested: table_stack.len() > 1, section: section.clone() });
                    }
                    b"w:tr" if !is_empty => {
                        if let Some(t) = table_stack.last_mut() {
                            row_depth_first.push(t.rows == 0);
                            t.rows += 1;
                        }
                    }
                    b"w:trPr" if !is_empty => in_trpr = true,
                    b"w:tblHeader" if in_trpr => {
                        if row_depth_first.last() == Some(&true) {
                            if let Some(t) = table_stack.last_mut() {
                                t.header = on_off(e);
                            }
                        }
                    }
                    b"w:drawing" if !is_empty => {
                        drawing_stack.push(None);
                        if let Some((_, p)) = para_stack.last_mut() {
                            p.has_drawing = true;
                        }
                    }
                    b"wp:docPr" => {
                        let para = para_stack.last().map(|(i, _)| *i).unwrap_or(paras.len());
                        let img = ImageInfo {
                            index: images.len(),
                            id: attr(e, b"id").unwrap_or_default(),
                            name: attr(e, b"name").unwrap_or_default(),
                            alt: attr(e, b"descr").unwrap_or_default(),
                            title: attr(e, b"title").unwrap_or_default(),
                            kind: "form".into(),
                            section: section.clone(),
                            para,
                            ..Default::default()
                        };
                        if let Some(slot) = drawing_stack.last_mut() {
                            *slot = Some(images.len());
                        }
                        images.push(img);
                        if !is_empty {
                            in_docpr = true;
                        }
                    }
                    b"adec:decorative" if in_docpr => {
                        if let Some(img) = images.last_mut() {
                            img.decorative = attr(e, b"val").map(|v| v == "1" || v == "true").unwrap_or(false);
                        }
                    }
                    b"a:blip" => {
                        if let Some(Some(i)) = drawing_stack.last() {
                            let img = &mut images[*i];
                            img.kind = "bild".into();
                            if img.media.is_none() {
                                img.media = attr(e, b"r:embed").and_then(|id| rels.get(&id).cloned());
                            }
                        }
                    }
                    b"c:chart" => {
                        if let Some(Some(i)) = drawing_stack.last() {
                            images[*i].kind = "diagramm".into();
                        }
                    }
                    b"a:graphicData" if attr(e, b"uri").is_some_and(|u| u.ends_with("/diagram")) => {
                        if let Some(Some(i)) = drawing_stack.last() {
                            images[*i].kind = "smartart".into();
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                if in_t {
                    let s = t.unescape()?;
                    if let Some((_, p)) = para_stack.last_mut() {
                        p.text.push_str(&s);
                        if !s.trim().is_empty() {
                            run_has_text = true;
                        }
                    }
                    if let Some(h) = in_hyperlink.as_mut() {
                        h.push_str(&s);
                    }
                    if let Some(t) = table_stack.last_mut() {
                        if row_depth_first.last() == Some(&true) && t.first_row_text.chars().count() < 200 {
                            t.first_row_text.push_str(&s);
                        }
                    }
                }
            }
            Event::End(e) => match e.name().as_ref() {
                b"w:t" => in_t = false,
                b"w:rPr" => in_rpr = false,
                b"w:r" => {
                    if run_has_text {
                        if let Some((_, p)) = para_stack.last_mut() {
                            p.text_runs += 1;
                            let style_bold = styles.is_bold(&p.style);
                            if run_bold.unwrap_or(style_bold) {
                                p.bold_runs += 1;
                            }
                        }
                    }
                }
                b"w:hyperlink" => {
                    if let Some(h) = in_hyperlink.take() {
                        let h = h.trim().to_lowercase();
                        if h.starts_with("http") || h.starts_with("www.") {
                            url_links += 1;
                        }
                    }
                }
                b"w:tc" => {
                    if let Some(t) = table_stack.last_mut() {
                        if row_depth_first.last() == Some(&true) && !t.first_row_text.ends_with(" | ") && !t.first_row_text.is_empty() {
                            t.first_row_text.push_str(" | ");
                        }
                    }
                }
                b"w:trPr" => in_trpr = false,
                b"w:tr" => {
                    row_depth_first.pop();
                }
                b"w:tbl" => {
                    if let Some(t) = table_stack.pop() {
                        let info = &mut tables[t.index];
                        info.header = t.header;
                        info.rows = t.rows;
                        info.first_row = snippet(t.first_row_text.trim_end_matches(" | "), 160);
                    }
                }
                b"wp:docPr" => in_docpr = false,
                b"w:drawing" => {
                    drawing_stack.pop();
                }
                b"w:p" => {
                    if let Some((idx, p)) = para_stack.pop() {
                        let style = if p.style.is_empty() { styles.default_para.clone().unwrap_or_default() } else { p.style.clone() };
                        let text = p.text.trim().to_string();
                        paras[idx] = ParaRecord { style: style.clone(), text: text.clone() };

                        let level = match p.outline {
                            Some(l) if l < 9 => Some(l + 1),
                            Some(_) => None,
                            None => styles.heading_level(&style),
                        };

                        // leere Absätze als Abstandhalter
                        let empty = text.is_empty() && !p.has_drawing && !p.has_break && !p.in_table;
                        if empty {
                            empty_streak += 1;
                        } else {
                            if empty_streak >= 2 {
                                hints.push(Hint {
                                    severity: "hinweis".into(),
                                    kind: "leer".into(),
                                    text: tr!(
                                        "{} leere Absätze hintereinander (nach „{}“). Abstände besser über die Formatvorlage einstellen.",
                                        "{} empty paragraphs in a row (after “{}”). Better set spacing in the paragraph style.",
                                        empty_streak,
                                        snippet(&empty_after, 60)
                                    ),
                                    section: section.clone(),
                                    ..Default::default()
                                });
                            }
                            empty_streak = 0;
                            if !text.is_empty() {
                                empty_after = text.clone();
                            }
                        }

                        if let Some(lvl) = level {
                            if !text.is_empty() {
                                if last_level > 0 && lvl > last_level + 1 {
                                    jumps.push((idx, lvl, last_level, snippet(&text, 120), section.clone()));
                                    hints.push(Hint {
                                        severity: "warnung".into(),
                                        kind: "ebene".into(),
                                        text: tr!("Überschrift Ebene {} folgt direkt auf Ebene {}: „{}“", "Heading level {} directly follows level {}: “{}”", lvl, last_level, snippet(&text, 80)),
                                        section: section.clone(),
                                        para: Some(idx),
                                    });
                                }
                                if last_level == 0 && lvl > 1 {
                                    hints.push(Hint {
                                        severity: "hinweis".into(),
                                        kind: "ebene".into(),
                                        text: tr!("Erste Überschrift hat Ebene {} statt 1: „{}“", "The first heading has level {} instead of 1: “{}”", lvl, snippet(&text, 80)),
                                        section: section.clone(),
                                        ..Default::default()
                                    });
                                }
                                last_level = lvl;
                                headings.push(HeadingInfo { level: lvl, text: snippet(&text, 120), para: idx, style: style.clone() });
                                section = snippet(&text, 120);
                            }
                        } else if !p.in_table && !styles.is_caption(&style) && !styles.is_toc_or_index(&style) {
                            let len = text.chars().count();
                            if p.text_runs > 0 && p.bold_runs == p.text_runs && (3..=100).contains(&len) && !text.ends_with('.') && !text.ends_with(':') {
                                hints.push(Hint {
                                    severity: "warnung".into(),
                                    kind: "scheinueberschrift".into(),
                                    text: tr!(
                                        "Fett gesetzter Absatz sieht aus wie eine Überschrift, ist aber keine: „{}“. Falls es eine Überschrift ist, Formatvorlage „Überschrift“ verwenden.",
                                        "Bold paragraph looks like a heading but is not one: “{}”. If it is a heading, use a “Heading” style.",
                                        snippet(&text, 80)
                                    ),
                                    section: section.clone(),
                                    ..Default::default()
                                });
                            }
                            let manual_list = ["- ", "• ", "– ", "* ", "▪ ", "· ", "o\t", "-\t", "•\t"].iter().any(|m| p.text.trim_start().starts_with(m));
                            if manual_list && !p.has_num {
                                hints.push(Hint {
                                    severity: "warnung".into(),
                                    kind: "liste".into(),
                                    text: tr!(
                                        "Aufzählung von Hand getippt: „{}“. Besser die Aufzählungs-Funktion bzw. -Formatvorlage verwenden.",
                                        "List typed by hand: “{}”. Better use the bullets/numbering feature or a list style.",
                                        snippet(&text, 80)
                                    ),
                                    section: section.clone(),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
        buf.clear();
    }

    // Bildunterschriften zuordnen
    for img in images.iter_mut() {
        let p = img.para;
        let candidates = (p..(p + 4).min(paras.len())).chain((p.saturating_sub(3)..p).rev());
        for j in candidates {
            if styles.is_caption(&paras[j].style) && !paras[j].text.is_empty() {
                img.caption = paras[j].text.clone();
                break;
            }
        }
    }

    // Vektorgrafiken mit Text
    for img in images.iter_mut().filter(|i| i.kind == "smartart" && !i.decorative) {
        let name = snippet(if img.caption.is_empty() { &img.name } else { &img.caption }, 80);
        let text = tr!(
            "Grafik „{name}“ ist eine SmartArt-Grafik mit Text. PAC gibt dafür eine Warnung aus („Possibly inappropriate use of a Figure“). Unproblematisch, wenn der Alternativtext die Texte der Grafik wiedergibt.",
            "Graphic “{name}” is a SmartArt graphic with text. PAC shows a warning for it (“Possibly inappropriate use of a Figure”). No problem if the alternative text contains the texts of the graphic."
        );
        hints.push(Hint { severity: "hinweis".into(), kind: "grafiktext".into(), text, section: img.section.clone(), ..Default::default() });
    }
    for img in images.iter_mut() {
        let Some(m) = img.media.as_deref() else { continue };
        let ext = m.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if ext != "emf" && ext != "wmf" {
            continue;
        }
        if let Some(data) = read_entry(docx, &format!("word/{m}"))? {
            img.vector_text = vector_text_count(&data);
        }
        if img.vector_text == 0 || img.decorative {
            continue;
        }
        let name = snippet(if img.caption.is_empty() { &img.name } else { &img.caption }, 80);
        if img.vector_text >= VECTOR_TABLE_TEXTS {
            let text = tr!(
                "Grafik „{name}“ enthält sehr viel Text ({} Textstellen) – vermutlich eine als Bild eingefügte Tabelle. Bitte als echte Word-Tabelle einfügen; als Bild ist der Inhalt für Screenreader kaum nutzbar und PAC warnt („Possibly inappropriate use of a Figure“).",
                "Graphic “{name}” contains a lot of text ({} text items) – probably a table inserted as an image. Please insert it as a real Word table; as an image the content is hardly usable with a screen reader and PAC warns (“Possibly inappropriate use of a Figure”).",
                img.vector_text
            );
            hints.push(Hint { severity: "warnung".into(), kind: "grafiktext".into(), text, section: img.section.clone(), ..Default::default() });
        } else {
            let text = tr!(
                "Grafik „{name}“ ist eine Vektorgrafik mit Text ({} Textstellen). PAC gibt dafür eine Warnung aus („Possibly inappropriate use of a Figure“). Unproblematisch, wenn der Alternativtext die Beschriftungen wiedergibt – Schriftfarben in der Grafik selbst auf Kontrast prüfen.",
                "Graphic “{name}” is a vector graphic with text ({} text items). PAC shows a warning for it (“Possibly inappropriate use of a Figure”). No problem if the alternative text contains the labels – check the text colours in the graphic itself for contrast.",
                img.vector_text
            );
            hints.push(Hint { severity: "hinweis".into(), kind: "grafiktext".into(), text, section: img.section.clone(), ..Default::default() });
        }
    }

    // Links auf lokale Dateien
    let local_ids = local_link_ids(&rels);
    if !local_ids.is_empty() {
        let texts = local_link_texts(&doc, &local_ids)?;
        if !texts.is_empty() {
            let mut ex: Vec<String> = texts.iter().take(3).map(|t| tr!("„{}“", "“{}”", snippet(t, 50))).collect();
            if texts.len() > 3 {
                ex.push("…".into());
            }
            let text = tr!(
                "{} Link(s) zeigen auf Dateien auf einem einzelnen Rechner (z. B. C:\\Users\\…\\Downloads) und führen ins Leere: {}. Im PDF macht Mimi normalen Text daraus; in Word den Link entfernen (Rechtsklick → Link entfernen).",
                "{} link(s) point to files on a single computer (e.g. C:\\Users\\…\\Downloads) and lead nowhere: {}. In the PDF Mimi turns them into normal text; remove the link in Word (right-click → Remove Hyperlink).",
                texts.len(),
                ex.join(", ")
            );
            hints.push(Hint { severity: "warnung".into(), kind: "link".into(), text, ..Default::default() });
        }
    }

    // Zusammenfassende Hinweise
    if title.is_empty() {
        let text = i18n::t("Kein Dokumenttitel gesetzt (Datei → Eigenschaften → Titel).", "No document title set (File → Properties → Title).").into();
        hints.insert(0, Hint { severity: "fehler".into(), kind: "titel".into(), text, ..Default::default() });
    }
    if styles.language.is_empty() {
        let text = i18n::t("Keine Dokumentsprache festgelegt.", "No document language set.").into();
        hints.insert(0, Hint { severity: "fehler".into(), kind: "sprache".into(), text, ..Default::default() });
    }
    if headings.is_empty() {
        let text = i18n::t(
            "Das Dokument enthält keine Überschriften mit Überschrift-Formatvorlagen. Ohne sie gibt es keine Gliederung und keine Lesezeichen im PDF.",
            "The document has no headings with heading styles. Without them there is no outline and there are no bookmarks in the PDF.",
        )
        .into();
        hints.insert(0, Hint { severity: "fehler".into(), kind: "ebene".into(), text, ..Default::default() });
    }
    if url_links > 0 {
        let text = tr!(
            "{} Link(s) zeigen die nackte Webadresse als Text. Ein sprechender Linktext ist besser verständlich (für PAC kein Fehler).",
            "{} link(s) show the bare web address as text. A meaningful link text is easier to understand (not an error for PAC).",
            url_links
        );
        hints.push(Hint { severity: "hinweis".into(), kind: "link".into(), text, ..Default::default() });
    }
    for t in tables.iter().filter(|t| t.rows <= 1) {
        let text = tr!(
            "Tabelle {} hat nur eine Zeile – wird sie nur fürs Layout verwendet? Dann besser ohne Tabelle gestalten.",
            "Table {} has only one row – is it only used for layout? Then better lay it out without a table.",
            t.index + 1
        );
        hints.push(Hint { severity: "hinweis".into(), kind: "tabelle".into(), text, section: t.section.clone(), ..Default::default() });
    }

    // Übersprungene Ebenen: passende Formatvorlage je Ebene suchen
    let mut level_styles: HashMap<u8, HashMap<String, usize>> = HashMap::new();
    for h in &headings {
        if styles.heading_level(&h.style) == Some(h.level) {
            *level_styles.entry(h.level).or_default().entry(h.style.clone()).or_default() += 1;
        }
    }
    let style_for_level = |lvl: u8| -> Option<String> {
        if let Some(m) = level_styles.get(&lvl) {
            // meistverwendete Vorlage dieser Ebene im Dokument
            let mut v: Vec<_> = m.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            return v.first().map(|(k, _)| (*k).clone());
        }
        let mut cands: Vec<&String> = styles.map.keys().filter(|id| styles.heading_level(id) == Some(lvl)).collect();
        cands.sort_by_key(|id| (!styles.map[*id].name.eq_ignore_ascii_case(&format!("heading {lvl}")), (*id).clone()));
        cands.first().map(|s| (*s).clone())
    };
    let heading_fixes: Vec<HeadingFix> = jumps
        .into_iter()
        .filter_map(|(para, from, prev, text, section)| {
            let to = prev + 1;
            let style = style_for_level(to)?;
            let style_name = styles.map.get(&style).map(|s| s.name.clone()).unwrap_or_default();
            Some(HeadingFix { para, text, from, to, style, style_name: display_style_name(&style_name), section })
        })
        .collect();

    let colors = color_issues(docx, &doc)?;
    let pdf_prep = pdf_prep_notes(docx)?;

    let links = hyperlink_texts(&doc)?;
    let links_missing = links.iter().filter(|(has, t)| !has && !t.is_empty()).count();
    let formulas = crate::formula::find(docx)?;
    Ok(Analysis { title, language: styles.language, images, tables, headings, hints, links_total: links.len(), links_missing, heading_fixes, colors, pdf_prep, formulas })
}

/// Word-interne englische Namen der eingebauten Vorlagen lesbar machen.
fn display_style_name(n: &str) -> String {
    let l = n.to_lowercase();
    if let Some(k) = l.strip_prefix("heading ") {
        return format!("Überschrift {k}");
    }
    n.to_string()
}

// ---------------------------------------------------------------------------
// Schriftfarben
// ---------------------------------------------------------------------------

struct ColorStyle {
    id: String,
    name: String,
    based_on: Option<String>,
    colors: Vec<String>,
}

fn color_attr(e: &BytesStart) -> Option<String> {
    let v = attr(e, b"w:val")?.to_ascii_uppercase();
    parse_hex(&v).map(|_| v)
}

/// Sucht Schriftfarben mit zu wenig Kontrast in verwendeten Formatvorlagen,
/// Aufzählungszeichen und direkter Formatierung.
fn color_issues(docx: &[u8], doc: &[u8]) -> Result<Vec<ColorIssue>> {
    // verwendete Vorlagen + direkte Farben im Text
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut direct: HashMap<String, usize> = HashMap::new();
    let mut used_nums: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut scan = |xml: &[u8], direct: &mut HashMap<String, usize>| -> Result<()> {
        let mut r = reader(xml);
        let mut buf = Vec::new();
        let mut in_rpr = 0usize;
        loop {
            match r.read_event_into(&mut buf)? {
                Event::Eof => break,
                Event::Start(e) if e.name().as_ref() == b"w:rPr" => in_rpr += 1,
                Event::End(e) if e.name().as_ref() == b"w:rPr" => in_rpr = in_rpr.saturating_sub(1),
                Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                    b"w:pStyle" | b"w:rStyle" | b"w:tblStyle" => {
                        if let Some(v) = attr(&e, b"w:val") {
                            used.insert(v);
                        }
                    }
                    b"w:numId" => {
                        if let Some(v) = attr(&e, b"w:val") {
                            used_nums.insert(v);
                        }
                    }
                    b"w:color" if in_rpr > 0 => {
                        if let Some(c) = color_attr(&e) {
                            *direct.entry(c).or_default() += 1;
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
            buf.clear();
        }
        Ok(())
    };
    scan(doc, &mut direct)?;
    for name in zip_names(docx)? {
        if (name.starts_with("word/header") || name.starts_with("word/footer") || name == "word/footnotes.xml" || name == "word/endnotes.xml") && name.ends_with(".xml") {
            if let Some(x) = read_entry(docx, &name)? {
                scan(&x, &mut direct)?;
            }
        }
    }

    // Vorlagen
    let mut cstyles: HashMap<String, ColorStyle> = HashMap::new();
    let mut default_para: Option<String> = None;
    if let Some(st) = read_entry(docx, "word/styles.xml")? {
        let mut r = reader(&st);
        let mut buf = Vec::new();
        let mut cur: Option<ColorStyle> = None;
        loop {
            match r.read_event_into(&mut buf)? {
                Event::Eof => break,
                Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                    b"w:style" => {
                        let id = attr(&e, b"w:styleId").unwrap_or_default();
                        if attr(&e, b"w:type").as_deref() == Some("paragraph") && attr(&e, b"w:default").as_deref() == Some("1") {
                            default_para = Some(id.clone());
                        }
                        cur = Some(ColorStyle { id, name: String::new(), based_on: None, colors: Vec::new() });
                    }
                    b"w:name" => {
                        if let Some(c) = cur.as_mut() {
                            c.name = attr(&e, b"w:val").unwrap_or_default();
                        }
                    }
                    b"w:basedOn" => {
                        if let Some(c) = cur.as_mut() {
                            c.based_on = attr(&e, b"w:val");
                        }
                    }
                    b"w:color" => {
                        if let (Some(c), Some(col)) = (cur.as_mut(), color_attr(&e)) {
                            c.colors.push(col);
                        }
                    }
                    _ => {}
                },
                Event::End(e) if e.name().as_ref() == b"w:style" => {
                    if let Some(c) = cur.take() {
                        cstyles.insert(c.id.clone(), c);
                    }
                }
                _ => {}
            }
            buf.clear();
        }
    }
    if let Some(d) = default_para {
        used.insert(d);
    }
    // Basisvorlagen der verwendeten Vorlagen gelten mit
    let mut stack: Vec<String> = used.iter().cloned().collect();
    while let Some(id) = stack.pop() {
        if let Some(b) = cstyles.get(&id).and_then(|s| s.based_on.clone()) {
            if used.insert(b.clone()) {
                stack.push(b);
            }
        }
    }

    let mut places: HashMap<String, Vec<String>> = HashMap::new();
    let mut ids: Vec<&String> = used.iter().collect();
    ids.sort();
    for id in ids {
        if let Some(s) = cstyles.get(id) {
            for c in &s.colors {
                let p = places.entry(c.clone()).or_default();
                let label = tr!("Formatvorlage „{}“", "Style “{}”", display_style_name(&s.name));
                if !p.contains(&label) {
                    p.push(label);
                }
            }
        }
    }
    // Aufzählungszeichen / Nummern
    if let Some(nb) = read_entry(docx, "word/numbering.xml")? {
        let mut abs_colors: HashMap<String, Vec<String>> = HashMap::new();
        let mut num_to_abs: HashMap<String, String> = HashMap::new();
        let mut r = reader(&nb);
        let mut buf = Vec::new();
        let mut cur_abs: Option<String> = None;
        let mut cur_num: Option<String> = None;
        loop {
            match r.read_event_into(&mut buf)? {
                Event::Eof => break,
                Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                    b"w:abstractNum" => cur_abs = attr(&e, b"w:abstractNumId"),
                    b"w:num" => cur_num = attr(&e, b"w:numId"),
                    b"w:abstractNumId" => {
                        if let (Some(n), Some(a)) = (cur_num.clone(), attr(&e, b"w:val")) {
                            num_to_abs.insert(n, a);
                        }
                    }
                    b"w:color" => {
                        if let (Some(a), Some(c)) = (cur_abs.clone(), color_attr(&e)) {
                            abs_colors.entry(a).or_default().push(c);
                        }
                    }
                    _ => {}
                },
                Event::End(e) => match e.name().as_ref() {
                    b"w:abstractNum" => cur_abs = None,
                    b"w:num" => cur_num = None,
                    _ => {}
                },
                _ => {}
            }
            buf.clear();
        }
        // Nummerierungen, die von verwendeten Vorlagen benutzt werden, sind schwer
        // vollständig aufzulösen – direkt im Text verwendete reichen für die Praxis.
        for n in &used_nums {
            if let Some(cols) = num_to_abs.get(n).and_then(|a| abs_colors.get(a)) {
                for c in cols {
                    let p = places.entry(c.clone()).or_default();
                    let label = i18n::t("Aufzählungszeichen/Nummerierung", "Bullets/numbering").to_string();
                    if !p.contains(&label) {
                        p.push(label);
                    }
                }
            }
        }
    }
    for (c, n) in &direct {
        places.entry(c.clone()).or_default().push(tr!("{n}× direkt im Text formatiert", "{n}× formatted directly in the text"));
    }

    let mut out: Vec<ColorIssue> = places
        .into_iter()
        .filter_map(|(c, places)| {
            let rgb = parse_hex(&c)?;
            let ratio = contrast_white(rgb);
            if ratio >= MIN_CONTRAST {
                return None;
            }
            let d = darken_to(rgb, TARGET_CONTRAST);
            let sugg = format!("{:02X}{:02X}{:02X}", d[0], d[1], d[2]);
            let sr = contrast_white([d[0] as f64 / 255.0, d[1] as f64 / 255.0, d[2] as f64 / 255.0]);
            Some(ColorIssue { color: c, ratio: (ratio * 100.0).floor() / 100.0, suggestion: sugg, suggestion_ratio: (sr * 100.0).floor() / 100.0, places })
        })
        .collect();
    out.sort_by(|a, b| a.ratio.partial_cmp(&b.ratio).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

fn zip_names(docx: &[u8]) -> Result<Vec<String>> {
    let zip = zip::ZipArchive::new(Cursor::new(docx))?;
    Ok(zip.file_names().map(|s| s.to_string()).collect())
}

// ---------------------------------------------------------------------------
// Änderungen schreiben
// ---------------------------------------------------------------------------

fn copy_start_with(e: &BytesStart, set: &[(&str, &str)]) -> BytesStart<'static> {
    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let mut out = BytesStart::new(name);
    for a in e.attributes().flatten() {
        let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        if set.iter().any(|(sk, _)| *sk == k) {
            continue;
        }
        let v = a.unescape_value().map(|v| v.into_owned()).unwrap_or_default();
        out.push_attribute((k.as_str(), v.as_str()));
    }
    for (k, v) in set {
        out.push_attribute((*k, *v));
    }
    out
}

fn write_decorative_ext(w: &mut Writer<Cursor<Vec<u8>>>, on: bool) -> Result<()> {
    let mut ext = BytesStart::new("a:ext");
    ext.push_attribute(("uri", DECORATIVE_URI));
    w.write_event(Event::Start(ext))?;
    let mut d = BytesStart::new("adec:decorative");
    d.push_attribute(("xmlns:adec", NS_ADEC));
    d.push_attribute(("val", if on { "1" } else { "0" }));
    w.write_event(Event::Empty(d))?;
    w.write_event(Event::End(BytesEnd::new("a:ext")))?;
    Ok(())
}

enum RowState {
    None,
    /// erste Zeile einer Zieltabelle, trPr noch nicht gesehen
    AwaitTrPr { want: bool, skip_depth: usize },
    InTrPr { want: bool, seen: bool, depth: usize, skipping: usize },
}

fn apply_document(xml: &[u8], changes: &Changes) -> Result<Vec<u8>> {
    let link_texts = if changes.link_descriptions { hyperlink_texts(xml)? } else { Vec::new() };
    let mut link_count = 0usize;
    let img_map: HashMap<usize, &ImageChange> = changes.images.iter().map(|c| (c.index, c)).collect();
    let tbl_map: HashMap<usize, bool> = changes.tables.iter().map(|c| (c.index, c.header)).collect();
    let head_map: HashMap<usize, &str> = changes.headings.iter().map(|c| (c.para, c.style.as_str())).collect();
    // Absatzzählung wie in analyze(): jedes w:p (auch leere) zählt
    let mut para_count = 0usize;
    let mut para_stack: Vec<usize> = Vec::new();

    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(xml.len() + 4096)));
    let mut buf = Vec::new();

    let mut docpr_count = 0usize;
    // aktuelles docPr: (Änderung, extLst gesehen, decorative gesehen, Tiefe im extLst)
    let mut docpr: Option<(&ImageChange, bool, bool)> = None;
    let mut in_docpr_extlst = false;

    let mut table_count = 0usize;
    let mut table_stack: Vec<(usize, usize)> = Vec::new(); // (Index, Zeilen bisher)
    let mut row = RowState::None;

    let tbl_header = || {
        let e = BytesStart::new("w:tblHeader");
        Event::Empty(e)
    };

    loop {
        let ev = r.read_event_into(&mut buf)?;
        if matches!(ev, Event::Eof) {
            break;
        }

        // --- Kopfzeilen-Logik für erste Tabellenzeilen ---
        match &mut row {
            RowState::AwaitTrPr { want, skip_depth } => {
                let want = *want;
                if *skip_depth > 0 {
                    match &ev {
                        Event::Start(_) => *skip_depth += 1,
                        Event::End(_) => *skip_depth -= 1,
                        _ => {}
                    }
                    w.write_event(ev)?;
                    buf.clear();
                    continue;
                }
                match &ev {
                    Event::Start(e) if e.name().as_ref() == b"w:tblPrEx" => {
                        *skip_depth = 1;
                        w.write_event(ev)?;
                        buf.clear();
                        continue;
                    }
                    Event::Empty(e) if e.name().as_ref() == b"w:tblPrEx" => {
                        w.write_event(ev)?;
                        buf.clear();
                        continue;
                    }
                    Event::Start(e) if e.name().as_ref() == b"w:trPr" => {
                        w.write_event(ev)?;
                        row = RowState::InTrPr { want, seen: false, depth: 0, skipping: 0 };
                        buf.clear();
                        continue;
                    }
                    Event::Empty(e) if e.name().as_ref() == b"w:trPr" => {
                        w.write_event(Event::Start(BytesStart::new("w:trPr")))?;
                        if want {
                            w.write_event(tbl_header())?;
                        }
                        w.write_event(Event::End(BytesEnd::new("w:trPr")))?;
                        row = RowState::None;
                        buf.clear();
                        continue;
                    }
                    Event::Start(_) | Event::Empty(_) | Event::End(_) => {
                        if want {
                            w.write_event(Event::Start(BytesStart::new("w:trPr")))?;
                            w.write_event(tbl_header())?;
                            w.write_event(Event::End(BytesEnd::new("w:trPr")))?;
                        }
                        row = RowState::None;
                        // Ereignis unten normal weiterverarbeiten
                    }
                    _ => {
                        w.write_event(ev)?;
                        buf.clear();
                        continue;
                    }
                }
            }
            RowState::InTrPr { want, seen, depth, skipping } => {
                if *skipping > 0 {
                    match &ev {
                        Event::Start(_) => *skipping += 1,
                        Event::End(_) => *skipping -= 1,
                        _ => {}
                    }
                    buf.clear();
                    continue;
                }
                match &ev {
                    Event::Start(e) | Event::Empty(e) if *depth == 0 && e.name().as_ref() == b"w:tblHeader" => {
                        if *want && !*seen {
                            w.write_event(tbl_header())?;
                            *seen = true;
                        }
                        if matches!(ev, Event::Start(_)) {
                            *skipping = 1;
                        }
                        buf.clear();
                        continue;
                    }
                    Event::Start(_) => *depth += 1,
                    Event::End(e) if *depth == 0 && e.name().as_ref() == b"w:trPr" => {
                        if *want && !*seen {
                            w.write_event(tbl_header())?;
                        }
                        w.write_event(ev)?;
                        row = RowState::None;
                        buf.clear();
                        continue;
                    }
                    Event::End(_) => *depth -= 1,
                    _ => {}
                }
                w.write_event(ev)?;
                buf.clear();
                continue;
            }
            RowState::None => {}
        }

        match &ev {
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"wp:docPr" => {
                let idx = docpr_count;
                docpr_count += 1;
                if let Some(ch) = img_map.get(&idx) {
                    let start = copy_start_with(e, &[("descr", ch.alt.as_str())]);
                    if matches!(ev, Event::Empty(_)) {
                        if ch.decorative {
                            w.write_event(Event::Start(start))?;
                            let mut lst = BytesStart::new("a:extLst");
                            lst.push_attribute(("xmlns:a", NS_A));
                            w.write_event(Event::Start(lst))?;
                            write_decorative_ext(&mut w, true)?;
                            w.write_event(Event::End(BytesEnd::new("a:extLst")))?;
                            w.write_event(Event::End(BytesEnd::new("wp:docPr")))?;
                        } else {
                            w.write_event(Event::Empty(start))?;
                        }
                    } else {
                        w.write_event(Event::Start(start))?;
                        docpr = Some((ch, false, false));
                    }
                    buf.clear();
                    continue;
                }
            }
            Event::Start(e) if docpr.is_some() && e.name().as_ref() == b"a:extLst" => {
                in_docpr_extlst = true;
                if let Some(d) = docpr.as_mut() {
                    d.1 = true;
                }
            }
            Event::Start(e) | Event::Empty(e) if docpr.is_some() && e.name().as_ref() == b"adec:decorative" => {
                let on = docpr.map(|d| d.0.decorative).unwrap_or(false);
                if let Some(d) = docpr.as_mut() {
                    d.2 = true;
                }
                let ne = copy_start_with(e, &[("val", if on { "1" } else { "0" })]);
                if matches!(ev, Event::Empty(_)) {
                    w.write_event(Event::Empty(ne))?;
                } else {
                    w.write_event(Event::Start(ne))?;
                }
                buf.clear();
                continue;
            }
            Event::End(e) if in_docpr_extlst && e.name().as_ref() == b"a:extLst" => {
                in_docpr_extlst = false;
                if let Some((ch, _, seen_dec)) = docpr {
                    if ch.decorative && !seen_dec {
                        write_decorative_ext(&mut w, true)?;
                        if let Some(d) = docpr.as_mut() {
                            d.2 = true;
                        }
                    }
                }
            }
            Event::End(e) if e.name().as_ref() == b"wp:docPr" => {
                if let Some((ch, seen_lst, _)) = docpr.take() {
                    if ch.decorative && !seen_lst {
                        let mut lst = BytesStart::new("a:extLst");
                        lst.push_attribute(("xmlns:a", NS_A));
                        w.write_event(Event::Start(lst))?;
                        write_decorative_ext(&mut w, true)?;
                        w.write_event(Event::End(BytesEnd::new("a:extLst")))?;
                    }
                }
            }
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"w:hyperlink" => {
                let idx = link_count;
                link_count += 1;
                if let Some((false, text)) = link_texts.get(idx) {
                    if !text.is_empty() {
                        let ne = copy_start_with(e, &[("w:tooltip", text.as_str())]);
                        w.write_event(if matches!(ev, Event::Empty(_)) { Event::Empty(ne) } else { Event::Start(ne) })?;
                        buf.clear();
                        continue;
                    }
                }
            }
            Event::Start(e) if e.name().as_ref() == b"w:p" => {
                para_stack.push(para_count);
                para_count += 1;
            }
            Event::Empty(e) if e.name().as_ref() == b"w:p" => {
                para_count += 1;
            }
            Event::End(e) if e.name().as_ref() == b"w:p" => {
                para_stack.pop();
            }
            Event::Empty(e) if e.name().as_ref() == b"w:pStyle" => {
                if let Some(style) = para_stack.last().and_then(|i| head_map.get(i)) {
                    w.write_event(Event::Empty(copy_start_with(e, &[("w:val", style)])))?;
                    buf.clear();
                    continue;
                }
            }
            Event::Start(e) if e.name().as_ref() == b"w:tbl" => {
                table_stack.push((table_count, 0));
                table_count += 1;
            }
            Event::End(e) if e.name().as_ref() == b"w:tbl" => {
                table_stack.pop();
            }
            Event::Start(e) if e.name().as_ref() == b"w:tr" => {
                if let Some((ti, rows)) = table_stack.last_mut() {
                    *rows += 1;
                    if *rows == 1 {
                        if let Some(want) = tbl_map.get(ti) {
                            w.write_event(ev)?;
                            row = RowState::AwaitTrPr { want: *want, skip_depth: 0 };
                            buf.clear();
                            continue;
                        }
                    }
                }
            }
            _ => {}
        }
        w.write_event(ev)?;
        buf.clear();
    }
    Ok(w.into_inner().into_inner())
}

fn apply_core(xml: &[u8], title: &str) -> Result<Vec<u8>> {
    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::new()));
    let mut buf = Vec::new();
    let mut inside = false;
    let mut done = false;
    loop {
        let ev = r.read_event_into(&mut buf)?;
        match &ev {
            Event::Eof => break,
            Event::Start(e) if e.name().as_ref() == b"dc:title" => {
                inside = true;
                w.write_event(ev.borrow())?;
                w.write_event(Event::Text(BytesText::new(title)))?;
                done = true;
                buf.clear();
                continue;
            }
            Event::Empty(e) if e.name().as_ref() == b"dc:title" => {
                w.write_event(Event::Start(BytesStart::new("dc:title")))?;
                w.write_event(Event::Text(BytesText::new(title)))?;
                w.write_event(Event::End(BytesEnd::new("dc:title")))?;
                done = true;
                buf.clear();
                continue;
            }
            Event::Text(_) if inside => {
                buf.clear();
                continue;
            }
            Event::End(e) if e.name().as_ref() == b"dc:title" => inside = false,
            Event::End(e) if e.name().as_ref() == b"cp:coreProperties" && !done => {
                let mut s = BytesStart::new("dc:title");
                s.push_attribute(("xmlns:dc", "http://purl.org/dc/elements/1.1/"));
                w.write_event(Event::Start(s))?;
                w.write_event(Event::Text(BytesText::new(title)))?;
                w.write_event(Event::End(BytesEnd::new("dc:title")))?;
            }
            _ => {}
        }
        w.write_event(ev)?;
        buf.clear();
    }
    Ok(w.into_inner().into_inner())
}

fn apply_language(xml: &[u8], lang: &str) -> Result<Vec<u8>> {
    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::new()));
    let mut buf = Vec::new();
    let mut in_default = false;
    let mut in_rpr = false;
    let mut seen = false;
    loop {
        let ev = r.read_event_into(&mut buf)?;
        match &ev {
            Event::Eof => break,
            Event::Start(e) if e.name().as_ref() == b"w:rPrDefault" => in_default = true,
            Event::End(e) if e.name().as_ref() == b"w:rPrDefault" => in_default = false,
            Event::Start(e) if in_default && e.name().as_ref() == b"w:rPr" => in_rpr = true,
            Event::Empty(e) if in_default && e.name().as_ref() == b"w:lang" => {
                seen = true;
                w.write_event(Event::Empty(copy_start_with(e, &[("w:val", lang)])))?;
                buf.clear();
                continue;
            }
            Event::End(e) if in_rpr && e.name().as_ref() == b"w:rPr" => {
                in_rpr = false;
                if !seen {
                    let mut l = BytesStart::new("w:lang");
                    l.push_attribute(("w:val", lang));
                    w.write_event(Event::Empty(l))?;
                    seen = true;
                }
            }
            _ => {}
        }
        w.write_event(ev)?;
        buf.clear();
    }
    Ok(w.into_inner().into_inner())
}

// ---------------------------------------------------------------------------
// Vorbereitung für den PDF-Export
// ---------------------------------------------------------------------------

/// Benennt eigene Formatvorlagen um, deren Name ein PDF-Standard-Tag ist.
/// Nur der Anzeigename (w:name) ändert sich, die ID und alle Verweise bleiben.
fn rename_std_styles(xml: &[u8]) -> Result<(Vec<u8>, Vec<String>)> {
    // vorhandene Namen sammeln, um Doppelungen zu vermeiden
    let mut names = std::collections::HashSet::new();
    {
        let mut r = reader(xml);
        let mut buf = Vec::new();
        loop {
            match r.read_event_into(&mut buf)? {
                Event::Eof => break,
                Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"w:name" => {
                    if let Some(v) = attr(&e, b"w:val") {
                        names.insert(v);
                    }
                }
                _ => {}
            }
            buf.clear();
        }
    }
    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(xml.len() + 256)));
    let mut buf = Vec::new();
    let mut custom = false;
    let mut renamed = Vec::new();
    loop {
        let ev = r.read_event_into(&mut buf)?;
        match &ev {
            Event::Eof => break,
            // auch ohne w:customStyle: LibreOffice lässt die Kennzeichnung beim Umwandeln aus ODT weg,
            // macht aber trotzdem aus jedem Vorlagennamen einen Tag. Umbenannt wird nur in der Export-Kopie.
            Event::Start(e) if e.name().as_ref() == b"w:style" => custom = true,
            Event::End(e) if e.name().as_ref() == b"w:style" => custom = false,
            Event::Empty(e) if custom && e.name().as_ref() == b"w:name" => {
                if let Some(v) = attr(e, b"w:val") {
                    if PDF_STD_TYPES.contains(&v.as_str()) {
                        let mut new = format!("{v} Vorlage");
                        let mut i = 2;
                        while names.contains(&new) {
                            new = format!("{v} Vorlage {i}");
                            i += 1;
                        }
                        names.insert(new.clone());
                        renamed.push(tr!(
                            "Formatvorlage „{v}“ heißt im PDF „{new}“, weil „{v}“ ein reservierter PDF-Tag ist (sonst werden z. B. alle Tabellenzellen falsch getaggt).",
                            "Style “{v}” is called “{new}” in the PDF because “{v}” is a reserved PDF tag (otherwise e.g. all table cells would be tagged wrongly)."
                        ));
                        w.write_event(Event::Empty(copy_start_with(e, &[("w:val", new.as_str())])))?;
                        buf.clear();
                        continue;
                    }
                }
            }
            _ => {}
        }
        w.write_event(ev)?;
        buf.clear();
    }
    Ok((w.into_inner().into_inner(), renamed))
}

/// Entfernt Verknüpfungen (Hyperlinks und Verweis-Felder wie STYLEREF) aus
/// Kopf- und Fußzeilen. Der angezeigte Text bleibt stehen. Kopf-/Fußzeilen sind
/// im PDF Artefakte; Links darin verletzen PDF/UA 7.18.5.
/// Zusammengesetzte Felder (`w:fldChar` begin … `w:instrText` … separate … Ergebnis … end),
/// deren Befehl `is_target` erfüllt, zu Text machen: Feldmarken und Befehl fallen weg, das
/// angezeigte Ergebnis bleibt. Rückgabe: Anzahl aufgelöster Felder.
fn unlink_complex_fields(xml: &[u8], is_target: &dyn Fn(&str) -> bool) -> Result<(Vec<u8>, usize)> {
    // 1. Durchgang: Befehl jedes Felds (in Reihenfolge seines Beginns) bestimmen
    let mut targets: Vec<bool> = Vec::new();
    {
        let mut r = reader(xml);
        let mut buf = Vec::new();
        let mut stack: Vec<(usize, bool, String)> = Vec::new(); // (Feld, im Befehlsteil, Befehl)
        let mut in_instr = false;
        loop {
            match r.read_event_into(&mut buf)? {
                Event::Eof => break,
                Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"w:fldChar" => match attr(&e, b"w:fldCharType").as_deref() {
                    Some("begin") => {
                        stack.push((targets.len(), true, String::new()));
                        targets.push(false);
                    }
                    Some("separate") => {
                        if let Some(top) = stack.last_mut() {
                            top.1 = false;
                            targets[top.0] = is_target(&top.2);
                        }
                    }
                    Some("end") => {
                        if let Some((i, instr_phase, instr)) = stack.pop() {
                            if instr_phase {
                                targets[i] = is_target(&instr);
                            }
                        }
                    }
                    _ => {}
                },
                Event::Start(e) if e.name().as_ref() == b"w:instrText" => in_instr = true,
                Event::End(e) if e.name().as_ref() == b"w:instrText" => in_instr = false,
                Event::Text(t) if in_instr => {
                    if let Some(top) = stack.last_mut() {
                        if top.1 {
                            top.2.push_str(&t.unescape().unwrap_or_default());
                        }
                    }
                }
                _ => {}
            }
            buf.clear();
        }
    }
    let n = targets.iter().filter(|t| **t).count();
    if n == 0 {
        return Ok((xml.to_vec(), 0));
    }
    // 2. Durchgang: Feldmarken und Befehl der markierten Felder weglassen
    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(xml.len())));
    let mut buf = Vec::new();
    let mut field_no = 0usize;
    let mut stack: Vec<(bool, bool)> = Vec::new(); // (auflösen, im Befehlsteil)
    let mut skip_depth = 0usize;
    loop {
        let ev = r.read_event_into(&mut buf)?;
        if skip_depth > 0 {
            match &ev {
                Event::Start(_) => skip_depth += 1,
                Event::End(_) => skip_depth -= 1,
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
            continue;
        }
        let drop_instr = stack.last().is_some_and(|&(t, instr)| t && instr);
        let mut drop = false;
        match &ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"w:fldChar" => {
                let is_start = matches!(ev, Event::Start(_));
                match attr(e, b"w:fldCharType").as_deref() {
                    Some("begin") => {
                        let t = targets.get(field_no).copied().unwrap_or(false);
                        field_no += 1;
                        stack.push((t, true));
                        drop = t;
                    }
                    Some("separate") => {
                        if let Some(top) = stack.last_mut() {
                            top.1 = false;
                            drop = top.0;
                        }
                    }
                    Some("end") => {
                        if let Some((t, _)) = stack.pop() {
                            drop = t;
                        }
                    }
                    _ => drop = drop_instr,
                }
                if drop && is_start {
                    skip_depth = 1;
                }
            }
            Event::Start(e) if drop_instr && e.name().as_ref() == b"w:instrText" => {
                skip_depth = 1;
                drop = true;
            }
            Event::Empty(e) if drop_instr && e.name().as_ref() == b"w:instrText" => drop = true,
            _ => {}
        }
        if !drop {
            w.write_event(ev)?;
        }
        buf.clear();
    }
    Ok((w.into_inner().into_inner(), n))
}

fn unlink_header_footer(xml: &[u8]) -> Result<(Vec<u8>, usize)> {
    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(xml.len())));
    let mut buf = Vec::new();
    let mut fld_stack: Vec<bool> = Vec::new();
    let mut hl_depth = 0usize;
    let mut n = 0usize;
    loop {
        let ev = r.read_event_into(&mut buf)?;
        match &ev {
            Event::Eof => break,
            Event::Start(e) if e.name().as_ref() == b"w:fldSimple" => {
                let drop = field_is_link(&attr(e, b"w:instr").unwrap_or_default());
                fld_stack.push(drop);
                if drop {
                    n += 1;
                    buf.clear();
                    continue;
                }
            }
            Event::End(e) if e.name().as_ref() == b"w:fldSimple" => {
                if fld_stack.pop() == Some(true) {
                    buf.clear();
                    continue;
                }
            }
            Event::Start(e) if e.name().as_ref() == b"w:hyperlink" => {
                hl_depth += 1;
                n += 1;
                buf.clear();
                continue;
            }
            Event::End(e) if hl_depth > 0 && e.name().as_ref() == b"w:hyperlink" => {
                hl_depth -= 1;
                buf.clear();
                continue;
            }
            _ => {}
        }
        w.write_event(ev)?;
        buf.clear();
    }
    Ok((w.into_inner().into_inner(), n))
}

/// Benennt die Word-Vorlage „caption“ (deutsch „Beschriftung“) um. LibreOffice
/// taggt Absätze mit dieser Vorlage (und direkt davon abgeleitete) im PDF als
/// `Caption` – frei stehend, nicht innerhalb von Figure/Table, was PAC
/// bemängelt. Mit eigenem Namen werden sie normale Absätze (P);
/// Aussehen, Nummerierung (SEQ-Felder) und Verzeichnisse bleiben gleich.
fn rename_caption_style(xml: &[u8]) -> Result<(Vec<u8>, bool)> {
    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(xml.len() + 64)));
    let mut buf = Vec::new();
    let mut in_style = false;
    let mut done = false;
    loop {
        let ev = r.read_event_into(&mut buf)?;
        match &ev {
            Event::Eof => break,
            Event::Start(e) if e.name().as_ref() == b"w:style" => in_style = true,
            Event::End(e) if e.name().as_ref() == b"w:style" => in_style = false,
            Event::Empty(e) if in_style && e.name().as_ref() == b"w:name" => {
                if attr(e, b"w:val").is_some_and(|v| v.eq_ignore_ascii_case("caption")) {
                    w.write_event(Event::Empty(copy_start_with(e, &[("w:val", CAPTION_NEW_NAME)])))?;
                    done = true;
                    buf.clear();
                    continue;
                }
            }
            _ => {}
        }
        w.write_event(ev)?;
        buf.clear();
    }
    Ok((w.into_inner().into_inner(), done))
}

const CAPTION_NEW_NAME: &str = "Bildunterschrift (Mimi)";

/// Feldbefehle für Literaturverzeichnisse: Word (BIBLIOGRAPHY), Citavi, Zotero, EndNote.
fn is_bibliography_instr(instr: &str) -> bool {
    let up = instr.trim().to_ascii_uppercase();
    let first = up.split_whitespace().next().unwrap_or("");
    first == "BIBLIOGRAPHY" || (first == "ADDIN" && (up.contains("BIBL") || up.contains("REFLIST")))
}

/// Macht Literaturverzeichnis-Felder zu normalem Text: Feldklammern und
/// Feldbefehl fallen weg, das angezeigte Ergebnis bleibt. Ein umgebendes
/// Inhaltssteuerelement mit Katalog „Bibliographies“ wird ebenfalls aufgelöst.
/// Sonst baut LibreOffice daraus ein geschütztes Verzeichnis, das anders
/// aussieht als in Word.
fn unlink_bibliography(xml: &[u8]) -> Result<(Vec<u8>, usize)> {
    // 1. Durchgang: welche Felder (in Reihenfolge ihres Beginns) und welche
    //    Inhaltssteuerelemente sind Literaturverzeichnisse?
    let mut bib_fields: Vec<bool> = Vec::new();
    let mut bib_sdts: Vec<bool> = Vec::new();
    {
        let mut r = reader(xml);
        let mut buf = Vec::new();
        let mut stack: Vec<(usize, bool, String)> = Vec::new(); // (Feld, im Befehlsteil, Befehl)
        let mut sdt_stack: Vec<usize> = Vec::new();
        let mut in_instr = false;
        loop {
            match r.read_event_into(&mut buf)? {
                Event::Eof => break,
                Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"w:fldChar" => {
                    match attr(&e, b"w:fldCharType").as_deref() {
                        Some("begin") => {
                            stack.push((bib_fields.len(), true, String::new()));
                            bib_fields.push(false);
                        }
                        Some("separate") => {
                            if let Some(top) = stack.last_mut() {
                                top.1 = false;
                                bib_fields[top.0] = is_bibliography_instr(&top.2);
                            }
                        }
                        Some("end") => {
                            if let Some((i, instr_phase, instr)) = stack.pop() {
                                if instr_phase {
                                    bib_fields[i] = is_bibliography_instr(&instr);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Event::Start(e) if e.name().as_ref() == b"w:instrText" => in_instr = true,
                Event::End(e) if e.name().as_ref() == b"w:instrText" => in_instr = false,
                Event::Text(t) if in_instr => {
                    if let Some(top) = stack.last_mut() {
                        if top.1 {
                            top.2.push_str(&t.unescape().unwrap_or_default());
                        }
                    }
                }
                Event::Start(e) if e.name().as_ref() == b"w:sdt" => {
                    sdt_stack.push(bib_sdts.len());
                    bib_sdts.push(false);
                }
                Event::End(e) if e.name().as_ref() == b"w:sdt" => {
                    sdt_stack.pop();
                }
                Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"w:docPartGallery" => {
                    if attr(&e, b"w:val").as_deref() == Some("Bibliographies") {
                        if let Some(&i) = sdt_stack.last() {
                            bib_sdts[i] = true;
                        }
                    }
                }
                _ => {}
            }
            buf.clear();
        }
    }
    let n = bib_fields.iter().filter(|b| **b).count();
    let n_sdt = bib_sdts.iter().filter(|b| **b).count();

    // 2. Durchgang: schreiben, dabei die markierten Teile weglassen.
    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(xml.len())));
    let mut buf = Vec::new();
    let mut field_no = 0usize;
    let mut stack: Vec<(bool, bool)> = Vec::new(); // (Literaturverzeichnis, im Befehlsteil)
    let mut simple_stack: Vec<bool> = Vec::new();
    let mut sdt_no = 0usize;
    let mut sdt_stack: Vec<bool> = Vec::new();
    let mut skip_depth = 0usize; // innerhalb eines weggelassenen Teilbaums
    let mut simple_n = 0usize;
    loop {
        let ev = r.read_event_into(&mut buf)?;
        if skip_depth > 0 {
            match &ev {
                Event::Start(_) => skip_depth += 1,
                Event::End(_) => skip_depth -= 1,
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
            continue;
        }
        let drop_instr = stack.last().is_some_and(|&(bib, instr)| bib && instr);
        let mut drop = false;
        match &ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"w:fldChar" => {
                let is_start = matches!(ev, Event::Start(_));
                match attr(e, b"w:fldCharType").as_deref() {
                    Some("begin") => {
                        let bib = bib_fields.get(field_no).copied().unwrap_or(false);
                        field_no += 1;
                        stack.push((bib, true));
                        drop = bib;
                    }
                    Some("separate") => {
                        if let Some(top) = stack.last_mut() {
                            top.1 = false;
                            drop = top.0;
                        }
                    }
                    Some("end") => {
                        if let Some((bib, _)) = stack.pop() {
                            drop = bib;
                        }
                    }
                    _ => drop = drop_instr,
                }
                if drop && is_start {
                    skip_depth = 1;
                }
            }
            Event::Start(e) if drop_instr && e.name().as_ref() == b"w:instrText" => {
                skip_depth = 1;
                drop = true;
            }
            Event::Empty(e) if drop_instr && e.name().as_ref() == b"w:instrText" => drop = true,
            Event::Start(e) if e.name().as_ref() == b"w:fldSimple" => {
                let bib = is_bibliography_instr(&attr(e, b"w:instr").unwrap_or_default());
                simple_stack.push(bib);
                if bib {
                    simple_n += 1;
                    drop = true;
                }
            }
            Event::End(e) if e.name().as_ref() == b"w:fldSimple" => drop = simple_stack.pop() == Some(true),
            Event::Start(e) if e.name().as_ref() == b"w:sdt" => {
                let bib = bib_sdts.get(sdt_no).copied().unwrap_or(false);
                sdt_no += 1;
                sdt_stack.push(bib);
                drop = bib;
            }
            Event::End(e) if e.name().as_ref() == b"w:sdt" => drop = sdt_stack.pop() == Some(true),
            Event::Start(e) if sdt_stack.last() == Some(&true) && matches!(e.name().as_ref(), b"w:sdtPr" | b"w:sdtEndPr") => {
                skip_depth = 1;
                drop = true;
            }
            Event::Empty(e) if sdt_stack.last() == Some(&true) && matches!(e.name().as_ref(), b"w:sdtPr" | b"w:sdtEndPr") => drop = true,
            _ => {}
        }
        // w:sdtContent eines aufgelösten Steuerelements: nur die Hülle weglassen
        if let Event::Start(e) = &ev {
            if e.name().as_ref() == b"w:sdtContent" && sdt_stack.last() == Some(&true) {
                drop = true;
            }
        }
        if let Event::End(e) = &ev {
            if e.name().as_ref() == b"w:sdtContent" && sdt_stack.last() == Some(&true) {
                drop = true;
            }
        }
        if !drop {
            w.write_event(ev)?;
        }
        buf.clear();
    }
    let total = n + simple_n;
    Ok((w.into_inner().into_inner(), if total > 0 { total } else { n_sdt }))
}

/// Zählt Word-Rahmen (aufeinanderfolgende Absätze mit `w:framePr` bilden einen Rahmen).
fn count_frames(xml: &[u8]) -> Result<usize> {
    let mut r = reader(xml);
    let mut buf = Vec::new();
    let (mut n, mut prev, mut cur) = (0usize, false, false);
    loop {
        match r.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) if e.name().as_ref() == b"w:p" => cur = false,
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"w:framePr" => cur = true,
            Event::End(e) if e.name().as_ref() == b"w:p" => {
                if cur && !prev {
                    n += 1;
                }
                prev = cur;
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(n)
}

fn is_header_footer(name: &str) -> bool {
    (name.starts_with("word/header") || name.starts_with("word/footer")) && name.ends_with(".xml") && !name.contains("/_rels/")
}

/// Bereitet eine Kopie der DOCX für den PDF-Export mit LibreOffice vor.
/// Die herunterladbare DOCX bleibt davon unberührt.
pub fn prepare_for_pdf(docx: &[u8]) -> Result<(Vec<u8>, Vec<String>)> {
    let mut replaced: HashMap<String, Vec<u8>> = HashMap::new();
    let mut notes = Vec::new();
    if let Some(st) = read_entry(docx, "word/styles.xml")? {
        let (new, ren) = rename_std_styles(&st)?;
        let changed = !ren.is_empty();
        notes.extend(ren);
        let (new2, cap) = rename_caption_style(&new)?;
        if cap {
            notes.push(
                i18n::t(
                    "Beschriftungen (Abbildung, Tabelle, Formel) werden im PDF als normale Absätze ausgegeben. LibreOffice würde sie sonst als frei stehende „Caption“ taggen, was PAC bemängelt.",
                    "Captions (figure, table, equation) are output as normal paragraphs in the PDF. Otherwise LibreOffice would tag them as stand-alone “Caption”, which PAC reports.",
                )
                .into(),
            );
        }
        if changed || cap {
            replaced.insert("word/styles.xml".into(), new2);
        }
    }
    if let Some(doc) = read_entry(docx, "word/document.xml")? {
        let frames = count_frames(&doc)?;
        if frames > 0 {
            notes.push(tr!(
                "{frames} Word-Rahmen (z. B. Titelblock, Impressum): LibreOffice legt ihren Inhalt im PDF in einen Absatz. Mimi löst ihn nach dem Export dort heraus – das Layout bleibt unverändert.",
                "{frames} Word frame(s) (e.g. title block, imprint): LibreOffice puts their content into a paragraph in the PDF. Mimi moves it out after the export – the layout stays the same."
            ));
        }
        let mut doc_changed = false;
        let (mut doc, n) = unlink_bibliography(&doc)?;
        if n > 0 {
            doc_changed = true;
            notes.push(
                i18n::t(
                    "Das Literaturverzeichnis wird im PDF als normaler Text ausgegeben – genau wie in Word. LibreOffice würde daraus sonst ein geschütztes Verzeichnis mit anderer Formatierung machen.",
                    "The bibliography is output as normal text in the PDF – just as in Word. Otherwise LibreOffice would turn it into a protected index with different formatting.",
                )
                .into(),
            );
        }
        if let Some(rels) = read_entry(docx, "word/_rels/document.xml.rels")? {
            let ids = local_link_ids(&parse_rels(&rels)?);
            if !ids.is_empty() {
                let (new, n) = unlink_local(&doc, &ids)?;
                if n > 0 {
                    doc = new;
                    doc_changed = true;
                    notes.push(tr!(
                        "{n} Link(s) auf Dateien auf einem einzelnen Rechner (z. B. C:\\Users\\…) werden im PDF als normaler Text ausgegeben – sie würden dort ins Leere führen.",
                        "{n} link(s) to files on a single computer (e.g. C:\\Users\\…) are output as normal text in the PDF – they would lead nowhere there."
                    ));
                }
            }
        }
        let (new, n) = mark_decorative_alt(&doc)?;
        if n > 0 {
            doc = new;
            doc_changed = true;
        }
        if let Some(st) = read_entry(docx, "word/styles.xml")? {
            let (new, n) = apply_table_spacing(&doc, &st)?;
            if n > 0 {
                doc = new;
                doc_changed = true;
                notes.push(tr!(
                    "Absatzabstände aus den Tabellenvorlagen werden in {n} Tabellenzellen übernommen, wie Word es macht. LibreOffice würde sonst die Standardabstände verwenden und die Tabellenzeilen höher machen.",
                    "Paragraph spacing from the table styles is applied in {n} table cells, as Word does. Otherwise LibreOffice would use the default spacing and make the table rows taller."
                ));
            }
        }
        if doc_changed {
            replaced.insert("word/document.xml".into(), doc);
        }
    }
    let mut links = 0;
    let mut deco = 0;
    for name in zip_names(docx)? {
        if is_header_footer(&name) {
            if let Some(x) = read_entry(docx, &name)? {
                let (new, n1) = unlink_header_footer(&x)?;
                // zusammengesetzte Verweis-Felder (so schreibt sie LibreOffice, je nach Version auch Word)
                let (new, n2) = unlink_complex_fields(&new, &field_is_link)?;
                let n = n1 + n2;
                let (new, d) = decorate_all(&new)?;
                if n > 0 || d > 0 {
                    links += n;
                    deco += d;
                    replaced.insert(name, new);
                }
            }
        }
    }
    if links > 0 {
        notes.push(tr!(
            "{links} Verknüpfung(en) in Kopf-/Fußzeilen (z. B. Feld STYLEREF) werden im PDF als normaler Text ausgegeben – sonst entsteht auf jeder Seite ein ungetaggter Link.",
            "{links} link(s) in headers/footers (e.g. STYLEREF field) are output as normal text in the PDF – otherwise every page gets an untagged link."
        ));
    }
    if deco > 0 {
        notes.push(tr!(
            "{deco} Bild(er) und Form(en) in Kopf-/Fußzeilen (Logos, Zierbalken) werden als dekorativ markiert. Kopf- und Fußzeilen liest der Screenreader ohnehin nicht vor; LibreOffice würde sie sonst als Abbildung taggen.",
            "{deco} image(s) and shape(s) in headers/footers (logos, ornamental bars) are marked as decorative. Screen readers do not read headers and footers anyway; otherwise LibreOffice would tag them as figures."
        ));
    }
    let ext = crate::sanitize::docx_external(docx, &mut replaced)?;
    if ext > 0 {
        notes.push(tr!(
            "{ext} verknüpfte(r) externe(r) Inhalt(e) (z. B. Bild per Link statt eingebettet, Vorlage auf einem anderen Rechner) werden nicht geladen. Im PDF fehlen sie – bitte in Word einfügen statt verknüpfen.",
            "{ext} linked external item(s) (e.g. an image linked instead of embedded, a template on another computer) are not loaded. They are missing from the PDF – please insert them in Word instead of linking."
        ));
    }
    if replaced.is_empty() {
        return Ok((docx.to_vec(), notes));
    }
    Ok((rewrite_zip(docx, &replaced)?, notes))
}

/// Nur die Hinweise, ohne die Datei zu bauen (für die Analyse).
fn pdf_prep_notes(docx: &[u8]) -> Result<Vec<String>> {
    Ok(prepare_for_pdf(docx)?.1)
}

pub(crate) fn rewrite_zip(docx: &[u8], replaced: &HashMap<String, Vec<u8>>) -> Result<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(docx))?;
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::with_capacity(docx.len())));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for i in 0..zip.len() {
        let name = zip.by_index_raw(i)?.name().to_string();
        if let Some(data) = replaced.get(&name) {
            out.start_file(name, opts)?;
            out.write_all(data)?;
        } else {
            let f = zip.by_index_raw(i)?;
            out.raw_copy_file(f)?;
        }
    }
    // neue Teile (z. B. docProps/custom.xml) anhängen
    let existing: std::collections::HashSet<String> = zip.file_names().map(str::to_string).collect();
    let mut added: Vec<&String> = replaced.keys().filter(|n| !existing.contains(*n)).collect();
    added.sort();
    for name in added {
        out.start_file(name.as_str(), opts)?;
        out.write_all(&replaced[name])?;
    }
    Ok(out.finish()?.into_inner())
}

/// Ersetzt Schriftfarben (w:color) und entfernt dabei Designfarben-Verweise,
/// die sonst Vorrang vor dem RGB-Wert hätten.
fn apply_colors(xml: &[u8], map: &HashMap<String, String>) -> Result<(Vec<u8>, usize)> {
    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(xml.len())));
    let mut buf = Vec::new();
    let mut n = 0;
    loop {
        let ev = r.read_event_into(&mut buf)?;
        match &ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"w:color" => {
                if let Some(to) = attr(e, b"w:val").and_then(|v| map.get(&v.to_ascii_uppercase())) {
                    let mut ne = BytesStart::new("w:color");
                    for a in e.attributes().flatten() {
                        match a.key.as_ref() {
                            b"w:val" => ne.push_attribute(("w:val", to.as_str())),
                            b"w:themeColor" | b"w:themeTint" | b"w:themeShade" => {}
                            _ => ne.push_attribute(a),
                        }
                    }
                    n += 1;
                    w.write_event(if matches!(ev, Event::Empty(_)) { Event::Empty(ne) } else { Event::Start(ne) })?;
                    buf.clear();
                    continue;
                }
            }
            _ => {}
        }
        w.write_event(ev)?;
        buf.clear();
    }
    Ok((w.into_inner().into_inner(), n))
}

/// Wendet die Änderungen an und liefert die neue DOCX-Datei.
pub fn apply(docx: &[u8], changes: &Changes) -> Result<Vec<u8>> {
    let mut replaced: HashMap<String, Vec<u8>> = HashMap::new();

    if !changes.images.is_empty() || !changes.tables.is_empty() || changes.link_descriptions || !changes.headings.is_empty() {
        let doc = read_entry(docx, "word/document.xml")?.context(i18n::t("word/document.xml fehlt", "word/document.xml is missing"))?;
        replaced.insert("word/document.xml".into(), apply_document(&doc, changes)?);
    }
    if let Some(t) = &changes.title {
        if let Some(core) = read_entry(docx, "docProps/core.xml")? {
            replaced.insert("docProps/core.xml".into(), apply_core(&core, t.trim())?);
        }
    }
    if let Some(l) = &changes.language {
        if !l.trim().is_empty() {
            if let Some(st) = read_entry(docx, "word/styles.xml")? {
                replaced.insert("word/styles.xml".into(), apply_language(&st, l.trim())?);
            }
        }
    }

    // Farben: auf alle Teile anwenden, die Schriftfarben enthalten können
    let color_map: HashMap<String, String> = changes
        .colors
        .iter()
        .filter_map(|c| {
            let f = c.from.trim().trim_start_matches('#').to_ascii_uppercase();
            let t = c.to.trim().trim_start_matches('#').to_ascii_uppercase();
            (parse_hex(&f).is_some() && parse_hex(&t).is_some()).then_some((f, t))
        })
        .collect();
    if !color_map.is_empty() {
        for name in zip_names(docx)? {
            let relevant = name == "word/document.xml" || name == "word/styles.xml" || name == "word/numbering.xml" || name == "word/footnotes.xml" || name == "word/endnotes.xml" || is_header_footer(&name);
            if !relevant {
                continue;
            }
            let src = match replaced.remove(&name) {
                Some(x) => x,
                None => match read_entry(docx, &name)? {
                    Some(x) => x,
                    None => continue,
                },
            };
            let (new, _) = apply_colors(&src, &color_map)?;
            replaced.insert(name, new);
        }
    }

    crate::formula::apply_alts(docx, &changes.formulas, &mut replaced)?;

    rewrite_zip(docx, &replaced)
}

// ---------------------------------------------------------------------------
// Export-Kopie: Abstände aus Tabellenvorlagen, Kopf-/Fußzeilenbilder, lokale Links
// ---------------------------------------------------------------------------

const SPACING_KEYS: [&str; 8] = ["w:before", "w:after", "w:line", "w:lineRule", "w:beforeLines", "w:afterLines", "w:beforeAutospacing", "w:afterAutospacing"];

/// Kinder von w:pPr, die laut Schema nach w:spacing kommen.
const AFTER_SPACING: [&[u8]; 14] = [
    b"w:ind", b"w:contextualSpacing", b"w:mirrorIndents", b"w:suppressOverlap", b"w:jc", b"w:textDirection", b"w:textAlignment",
    b"w:textboxTightWrap", b"w:outlineLvl", b"w:divId", b"w:cnfStyle", b"w:rPr", b"w:sectPr", b"w:pPrChange",
];

fn spacing_attrs(e: &BytesStart) -> Vec<(String, String)> {
    e.attributes()
        .flatten()
        .filter_map(|a| {
            let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
            SPACING_KEYS.contains(&k.as_str()).then(|| (k, a.unescape_value().map(|v| v.into_owned()).unwrap_or_default()))
        })
        .collect()
}

#[derive(Default)]
struct SpacingStyle {
    based_on: Option<String>,
    spacing: Vec<(String, String)>,
}

/// Absatzabstände der Formatvorlagen (nur w:pPr/w:spacing der Vorlage selbst,
/// nicht die bedingten Formate einer Tabellenvorlage).
struct SpacingStyles {
    map: HashMap<String, SpacingStyle>,
    default_para: Option<String>,
    default_table: Option<String>,
}

impl SpacingStyles {
    fn parse(xml: &[u8]) -> Result<Self> {
        let mut r = reader(xml);
        let mut buf = Vec::new();
        let mut map = HashMap::new();
        let (mut default_para, mut default_table) = (None, None);
        let mut cur: Option<(String, SpacingStyle)> = None;
        let mut cond = 0usize;
        let mut in_ppr = false;
        loop {
            let ev = r.read_event_into(&mut buf)?;
            match &ev {
                Event::Eof => break,
                Event::Start(e) | Event::Empty(e) => {
                    let empty = matches!(ev, Event::Empty(_));
                    match e.name().as_ref() {
                        b"w:style" => {
                            let id = attr(e, b"w:styleId").unwrap_or_default();
                            if attr(e, b"w:default").as_deref() == Some("1") {
                                match attr(e, b"w:type").as_deref() {
                                    Some("paragraph") => default_para = Some(id.clone()),
                                    Some("table") => default_table = Some(id.clone()),
                                    _ => {}
                                }
                            }
                            cur = Some((id, SpacingStyle::default()));
                        }
                        b"w:basedOn" => {
                            if let Some((_, s)) = cur.as_mut() {
                                s.based_on = attr(e, b"w:val");
                            }
                        }
                        b"w:tblStylePr" if !empty => cond += 1,
                        b"w:pPr" if !empty && cond == 0 => in_ppr = true,
                        b"w:spacing" if in_ppr && cond == 0 => {
                            if let Some((_, s)) = cur.as_mut() {
                                s.spacing = spacing_attrs(e);
                            }
                        }
                        _ => {}
                    }
                }
                Event::End(e) => match e.name().as_ref() {
                    b"w:style" => {
                        if let Some((id, s)) = cur.take() {
                            map.insert(id, s);
                        }
                        cond = 0;
                        in_ppr = false;
                    }
                    b"w:tblStylePr" => cond = cond.saturating_sub(1),
                    b"w:pPr" => in_ppr = false,
                    _ => {}
                },
                _ => {}
            }
            buf.clear();
        }
        Ok(SpacingStyles { map, default_para, default_table })
    }

    /// Abstände entlang der basedOn-Kette; die Vorlage `stop` (und alles darüber) zählt nicht.
    fn resolved(&self, id: &str, stop: Option<&str>) -> HashMap<String, String> {
        let mut chain = Vec::new();
        let mut cur = Some(id.to_string());
        while let Some(c) = cur {
            if chain.len() > 15 || stop == Some(c.as_str()) {
                break;
            }
            match self.map.get(&c) {
                Some(s) => {
                    chain.push(s);
                    cur = s.based_on.clone();
                }
                None => break,
            }
        }
        let mut out = HashMap::new();
        for s in chain.iter().rev() {
            for (k, v) in &s.spacing {
                out.insert(k.clone(), v.clone());
            }
        }
        out
    }
}

/// Word: Absatzabstände der Tabellenvorlage gelten in den Zellen, solange die
/// Absatzvorlage (außer „Standard“) oder direkte Formatierung sie nicht selbst
/// festlegt – sie stehen also über den Dokument-Standardwerten. LibreOffice nimmt
/// stattdessen die Dokument-Standardwerte (z. B. Zeilenabstand 1,25 statt 1,1),
/// wodurch Tabellenzeilen höher werden. Hier werden die fehlenden Werte der
/// Tabellenvorlage direkt an die Zellabsätze geschrieben.
fn apply_table_spacing(doc: &[u8], styles_xml: &[u8]) -> Result<(Vec<u8>, usize)> {
    let st = SpacingStyles::parse(styles_xml)?;
    let mut table_cache: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut r = reader(doc);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(doc.len() + 4096)));
    let mut buf = Vec::new();
    // je offener Tabelle: Abstände ihrer Vorlage
    let mut tables: Vec<HashMap<String, String>> = Vec::new();
    let mut txbx = 0usize;
    // Zustand des aktuellen Zellabsatzes: None = nichts zu tun
    // Some((Phase, pStyle)): 0 = w:pPr erwartet, 1 = in w:pPr (Tiefe), 2 = erledigt
    let mut phase: Option<(u8, usize, Option<String>)> = None;
    let mut n = 0usize;

    // fehlende Werte für einen Absatz berechnen und mit direkter Formatierung mischen
    let spacing_for = |table: &HashMap<String, String>, style: Option<&str>, direct: &[(String, String)]| -> Option<Vec<(String, String)>> {
        let style = style.map(str::to_string).or(st.default_para.clone()).unwrap_or_default();
        let para = st.resolved(&style, st.default_para.as_deref());
        let has = |k: &str| para.contains_key(k) || direct.iter().any(|(dk, _)| dk == k);
        let mut add = Vec::new();
        for (k, v) in table {
            let skip = match k.as_str() {
                "w:line" | "w:lineRule" => has("w:line"),
                k => has(k),
            };
            if !skip {
                add.push((k.clone(), v.clone()));
            }
        }
        if add.is_empty() {
            return None;
        }
        add.sort();
        let mut out = direct.to_vec();
        out.extend(add);
        Some(out)
    };
    let spacing_el = |attrs: &[(String, String)]| {
        let mut e = BytesStart::new("w:spacing");
        for (k, v) in attrs {
            e.push_attribute((k.as_str(), v.as_str()));
        }
        Event::Empty(e)
    };

    loop {
        let ev = r.read_event_into(&mut buf)?;
        if matches!(ev, Event::Eof) {
            break;
        }
        // Zellabsatz: w:pPr erwartet
        if let Some((0, _, _)) = phase {
            let table = tables.last().cloned().unwrap_or_default();
            match &ev {
                Event::Start(e) if e.name().as_ref() == b"w:pPr" => {
                    phase = Some((1, 0, None));
                    w.write_event(ev)?;
                    buf.clear();
                    continue;
                }
                Event::Empty(e) if e.name().as_ref() == b"w:pPr" => {
                    if let Some(sp) = spacing_for(&table, None, &[]) {
                        w.write_event(Event::Start(BytesStart::new("w:pPr")))?;
                        w.write_event(spacing_el(&sp))?;
                        w.write_event(Event::End(BytesEnd::new("w:pPr")))?;
                        n += 1;
                    } else {
                        w.write_event(ev)?;
                    }
                    phase = None;
                    buf.clear();
                    continue;
                }
                Event::Text(_) | Event::Comment(_) => {}
                _ => {
                    if let Some(sp) = spacing_for(&table, None, &[]) {
                        w.write_event(Event::Start(BytesStart::new("w:pPr")))?;
                        w.write_event(spacing_el(&sp))?;
                        w.write_event(Event::End(BytesEnd::new("w:pPr")))?;
                        n += 1;
                    }
                    phase = None;
                    // Ereignis unten normal weiterverarbeiten
                }
            }
        }
        // in w:pPr eines Zellabsatzes
        if let Some((1, depth, style)) = phase.as_mut() {
            let table = tables.last().cloned().unwrap_or_default();
            match &ev {
                Event::Start(e) | Event::Empty(e) if *depth == 0 => {
                    let name = e.name();
                    if name.as_ref() == b"w:pStyle" {
                        *style = attr(e, b"w:val");
                    } else if name.as_ref() == b"w:spacing" {
                        let direct = spacing_attrs(e);
                        if let Some(sp) = spacing_for(&table, style.as_deref(), &direct) {
                            let mut ne = BytesStart::new("w:spacing");
                            for a in e.attributes().flatten() {
                                if !SPACING_KEYS.contains(&String::from_utf8_lossy(a.key.as_ref()).as_ref()) {
                                    ne.push_attribute(a);
                                }
                            }
                            for (k, v) in &sp {
                                ne.push_attribute((k.as_str(), v.as_str()));
                            }
                            w.write_event(if matches!(ev, Event::Empty(_)) { Event::Empty(ne) } else { Event::Start(ne) })?;
                            n += 1;
                        } else {
                            w.write_event(ev.clone())?;
                        }
                        let started = matches!(ev, Event::Start(_));
                        phase = Some((2, if started { 1 } else { 0 }, None));
                        buf.clear();
                        continue;
                    } else if AFTER_SPACING.contains(&name.as_ref()) {
                        if let Some(sp) = spacing_for(&table, style.as_deref(), &[]) {
                            w.write_event(spacing_el(&sp))?;
                            n += 1;
                        }
                        phase = Some((2, 0, None));
                        // w:pPr-Ende unten nicht mehr behandeln; Tiefe wird nicht gebraucht
                        w.write_event(ev)?;
                        buf.clear();
                        continue;
                    }
                    if matches!(ev, Event::Start(_)) {
                        *depth += 1;
                    }
                }
                Event::Start(_) => *depth += 1,
                Event::End(e) if *depth == 0 && e.name().as_ref() == b"w:pPr" => {
                    if let Some(sp) = spacing_for(&table, style.as_deref(), &[]) {
                        w.write_event(spacing_el(&sp))?;
                        n += 1;
                    }
                    phase = None;
                    w.write_event(ev)?;
                    buf.clear();
                    continue;
                }
                Event::End(_) => *depth = depth.saturating_sub(1),
                _ => {}
            }
            w.write_event(ev)?;
            buf.clear();
            continue;
        }

        match &ev {
            Event::Start(e) if e.name().as_ref() == b"w:tbl" => {
                let def = st.default_table.clone();
                let sp = def.map(|d| st.resolved(&d, None)).unwrap_or_default();
                tables.push(sp);
            }
            Event::Empty(e) if e.name().as_ref() == b"w:tblStyle" => {
                if let (Some(top), Some(id)) = (tables.last_mut(), attr(e, b"w:val")) {
                    *top = table_cache.entry(id.clone()).or_insert_with(|| st.resolved(&id, None)).clone();
                }
            }
            Event::End(e) if e.name().as_ref() == b"w:tbl" => {
                tables.pop();
            }
            Event::Start(e) if e.name().as_ref() == b"w:txbxContent" => txbx += 1,
            Event::End(e) if e.name().as_ref() == b"w:txbxContent" => txbx = txbx.saturating_sub(1),
            Event::Start(e) if e.name().as_ref() == b"w:p" => {
                if txbx == 0 && tables.last().is_some_and(|t| !t.is_empty()) {
                    phase = Some((0, 0, None));
                } else {
                    phase = None;
                }
            }
            Event::End(e) if e.name().as_ref() == b"w:p" => phase = None,
            _ => {}
        }
        w.write_event(ev)?;
        buf.clear();
    }
    Ok((w.into_inner().into_inner(), n))
}

/// Markiert alle Bilder und Formen eines Teils (Kopf-/Fußzeile) als dekorativ.
/// Kopf- und Fußzeilen sind im PDF Artefakte; LibreOffice taggt verankerte
/// Grafiken darin aber trotzdem als `Figure` (PAC: „Possibly inappropriate use
/// of a Figure structure element“).
fn decorate_all(xml: &[u8]) -> Result<(Vec<u8>, usize)> {
    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(xml.len() + 1024)));
    let mut buf = Vec::new();
    // (extLst gesehen, decorative gesehen)
    let mut docpr: Option<(bool, bool)> = None;
    let mut in_lst = false;
    let mut n = 0usize;
    let ext_list = |w: &mut Writer<Cursor<Vec<u8>>>| -> Result<()> {
        let mut lst = BytesStart::new("a:extLst");
        lst.push_attribute(("xmlns:a", NS_A));
        w.write_event(Event::Start(lst))?;
        write_decorative_ext(w, true)?;
        w.write_event(Event::End(BytesEnd::new("a:extLst")))?;
        Ok(())
    };
    loop {
        let ev = r.read_event_into(&mut buf)?;
        match &ev {
            Event::Eof => break,
            Event::Empty(e) if e.name().as_ref() == b"wp:docPr" => {
                w.write_event(Event::Start(copy_start_with(e, &[("descr", DECORATIVE_ALT)])))?;
                ext_list(&mut w)?;
                w.write_event(Event::End(BytesEnd::new("wp:docPr")))?;
                n += 1;
                buf.clear();
                continue;
            }
            Event::Start(e) if e.name().as_ref() == b"wp:docPr" => {
                docpr = Some((false, false));
                w.write_event(Event::Start(copy_start_with(e, &[("descr", DECORATIVE_ALT)])))?;
                buf.clear();
                continue;
            }
            Event::Start(e) if docpr.is_some() && e.name().as_ref() == b"a:extLst" => {
                in_lst = true;
                if let Some(d) = docpr.as_mut() {
                    d.0 = true;
                }
            }
            Event::Start(e) | Event::Empty(e) if docpr.is_some() && e.name().as_ref() == b"adec:decorative" => {
                if let Some(d) = docpr.as_mut() {
                    d.1 = true;
                }
                if !matches!(attr(e, b"val").as_deref(), Some("1") | Some("true")) {
                    n += 1;
                }
                let ne = copy_start_with(e, &[("val", "1")]);
                w.write_event(if matches!(ev, Event::Empty(_)) { Event::Empty(ne) } else { Event::Start(ne) })?;
                buf.clear();
                continue;
            }
            Event::End(e) if in_lst && e.name().as_ref() == b"a:extLst" => {
                in_lst = false;
                if let Some((_, false)) = docpr {
                    write_decorative_ext(&mut w, true)?;
                    n += 1;
                    if let Some(d) = docpr.as_mut() {
                        d.1 = true;
                    }
                }
            }
            Event::End(e) if e.name().as_ref() == b"wp:docPr" => {
                if let Some((false, _)) = docpr.take() {
                    ext_list(&mut w)?;
                    n += 1;
                }
            }
            _ => {}
        }
        w.write_event(ev)?;
        buf.clear();
    }
    Ok((w.into_inner().into_inner(), n))
}

/// Gibt als dekorativ markierten Bildern und Formen im Fließtext den
/// Alternativtext [`DECORATIVE_ALT`], damit `pdffix` auch Formen ausblenden kann.
fn mark_decorative_alt(xml: &[u8]) -> Result<(Vec<u8>, usize)> {
    // 1. welche docPr (nach Reihenfolge) sind dekorativ?
    let mut deco = std::collections::HashSet::new();
    {
        let mut r = reader(xml);
        let mut buf = Vec::new();
        let (mut idx, mut inside) = (0usize, false);
        loop {
            match r.read_event_into(&mut buf)? {
                Event::Eof => break,
                Event::Start(e) if e.name().as_ref() == b"wp:docPr" => {
                    inside = true;
                    idx += 1;
                }
                Event::Empty(e) if e.name().as_ref() == b"wp:docPr" => idx += 1,
                Event::End(e) if e.name().as_ref() == b"wp:docPr" => inside = false,
                Event::Start(e) | Event::Empty(e) if inside && e.name().as_ref() == b"adec:decorative" => {
                    if matches!(attr(&e, b"val").as_deref(), Some("1") | Some("true")) {
                        deco.insert(idx - 1);
                    }
                }
                _ => {}
            }
            buf.clear();
        }
    }
    if deco.is_empty() {
        return Ok((xml.to_vec(), 0));
    }
    // 2. Alternativtext setzen
    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(xml.len() + 256)));
    let mut buf = Vec::new();
    let mut idx = 0usize;
    loop {
        let ev = r.read_event_into(&mut buf)?;
        match &ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"wp:docPr" => {
                let i = idx;
                idx += 1;
                if deco.contains(&i) {
                    let ne = copy_start_with(e, &[("descr", DECORATIVE_ALT)]);
                    w.write_event(if matches!(ev, Event::Empty(_)) { Event::Empty(ne) } else { Event::Start(ne) })?;
                    buf.clear();
                    continue;
                }
            }
            _ => {}
        }
        w.write_event(ev)?;
        buf.clear();
    }
    Ok((w.into_inner().into_inner(), deco.len()))
}

/// Linkziel, das nur auf einem einzelnen Rechner existiert (C:\…, file://, \\server\…).
fn is_local_target(t: &str) -> bool {
    let l = t.trim().to_ascii_lowercase();
    let b = l.as_bytes();
    l.starts_with("file:") || l.starts_with("\\\\") || (b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
}

fn local_link_ids(rels: &HashMap<String, String>) -> std::collections::HashSet<String> {
    rels.iter().filter(|(_, t)| is_local_target(t)).map(|(id, _)| id.clone()).collect()
}

/// Sichtbare Texte der Links auf lokale Dateien (für den Hinweis).
fn local_link_texts(doc: &[u8], ids: &std::collections::HashSet<String>) -> Result<Vec<String>> {
    let mut r = reader(doc);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut in_t = false;
    loop {
        match r.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) if e.name().as_ref() == b"w:hyperlink" => {
                cur = attr(&e, b"r:id").filter(|id| ids.contains(id)).map(|_| String::new());
            }
            Event::Start(e) if e.name().as_ref() == b"w:t" => in_t = true,
            Event::End(e) if e.name().as_ref() == b"w:t" => in_t = false,
            Event::Text(t) if in_t => {
                if let Some(c) = cur.as_mut() {
                    c.push_str(&t.unescape()?);
                }
            }
            Event::End(e) if e.name().as_ref() == b"w:hyperlink" => {
                if let Some(c) = cur.take() {
                    out.push(clean_link_text(&c));
                }
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// Löst Links auf lokale Dateien auf; der Text bleibt stehen.
fn unlink_local(doc: &[u8], ids: &std::collections::HashSet<String>) -> Result<(Vec<u8>, usize)> {
    let mut r = reader(doc);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(doc.len())));
    let mut buf = Vec::new();
    let mut stack: Vec<bool> = Vec::new();
    let mut n = 0usize;
    loop {
        let ev = r.read_event_into(&mut buf)?;
        match &ev {
            Event::Eof => break,
            Event::Start(e) if e.name().as_ref() == b"w:hyperlink" => {
                let drop = attr(e, b"r:id").is_some_and(|id| ids.contains(&id));
                stack.push(drop);
                if drop {
                    n += 1;
                    buf.clear();
                    continue;
                }
            }
            Event::End(e) if e.name().as_ref() == b"w:hyperlink" => {
                if stack.pop() == Some(true) {
                    buf.clear();
                    continue;
                }
            }
            _ => {}
        }
        w.write_event(ev)?;
        buf.clear();
    }
    Ok((w.into_inner().into_inner(), n))
}

/// Zählt Textausgaben in einer EMF- oder WMF-Grafik (0 bei anderen Formaten).
pub fn vector_text_count(data: &[u8]) -> usize {
    let u16at = |i: usize| data.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32at = |i: usize| data.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    // EMF: erster Datensatz EMR_HEADER, Signatur " EMF" an Byte 40
    if data.len() > 44 && u32at(0) == Some(1) && &data[40..44] == b" EMF" {
        let (mut i, mut gdi, mut plus) = (0usize, 0usize, 0usize);
        while let (Some(t), Some(sz)) = (u32at(i), u32at(i + 4)) {
            let sz = sz as usize;
            if sz < 8 || i + sz > data.len() {
                break;
            }
            match t {
                // EXTTEXTOUTA/W, POLYTEXTOUTA/W, SMALLTEXTOUT
                83 | 84 | 96 | 97 | 108 => gdi += 1,
                // EMR_COMMENT mit EMF+-Datensätzen
                70 if sz >= 16 && &data[i + 12..i + 16] == b"EMF+" => {
                    let mut j = i + 16;
                    let end = i + sz;
                    while j + 12 <= end {
                        let (Some(pt), Some(psz)) = (u16at(j), u32at(j + 4)) else { break };
                        if psz < 12 {
                            break;
                        }
                        // DrawString, DrawDriverString
                        if pt == 0x401C || pt == 0x4036 {
                            plus += 1;
                        }
                        j += psz as usize;
                    }
                }
                14 => break, // EMR_EOF
                _ => {}
            }
            i += sz;
        }
        // Dual-EMF enthält beides; nicht doppelt zählen
        return gdi.max(plus);
    }
    // WMF, ggf. mit „placeable“-Kopf
    let mut i = if u32at(0) == Some(0x9AC6_CDD7) { 22 } else { 0 };
    if !matches!(u16at(i), Some(1) | Some(2)) || u16at(i + 2) != Some(9) {
        return 0;
    }
    i += 18;
    let mut n = 0;
    while let (Some(sz), Some(f)) = (u32at(i), u16at(i + 4)) {
        if f == 0 || sz < 3 {
            break;
        }
        // META_TEXTOUT, META_EXTTEXTOUT
        if f == 0x0521 || f == 0x0A32 {
            n += 1;
        }
        i += sz as usize * 2;
    }
    n
}

/// Ab so vielen Textstellen ist eine Vektorgrafik vermutlich eine eingefügte Tabelle.
const VECTOR_TABLE_TEXTS: usize = 300;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:latentStyles><w:lsdException w:name="caption" w:semiHidden="1" w:unhideWhenUsed="1" w:qFormat="1"/></w:latentStyles>
<w:style w:type="paragraph" w:default="1" w:styleId="Standard"><w:name w:val="Normal"/><w:qFormat/></w:style>
<w:style w:type="paragraph" w:styleId="berschrift1"><w:name w:val="heading 1"/><w:basedOn w:val="Standard"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Beschriftung"><w:name w:val="caption"/><w:basedOn w:val="Standard"/><w:next w:val="Standard"/><w:qFormat/><w:rPr><w:i/><w:color w:val="44546A"/><w:sz w:val="18"/></w:rPr></w:style>
</w:styles>"#;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="berschrift1"/></w:pPr><w:r><w:t>Ergebnisse</w:t></w:r></w:p>
<w:p><w:r><w:t>Ein Absatz Text vor der Tabelle.</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="Beschriftung"/></w:pPr><w:r><w:t xml:space="preserve">Tabelle </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> SEQ Tabelle \* ARABIC </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>: Messwerte</w:t></w:r></w:p>
<w:tbl><w:tblPr><w:tblW w:w="0" w:type="auto"/></w:tblPr><w:tblGrid><w:gridCol w:w="4500"/><w:gridCol w:w="4500"/></w:tblGrid>
<w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:p><w:r><w:t>Probe</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>pH</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>6,8</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:pPr><w:pStyle w:val="berschrift1"/></w:pPr><w:r><w:t>Literatur</w:t></w:r></w:p>
<w:sdt><w:sdtPr><w:id w:val="-1"/><w:docPartObj><w:docPartGallery w:val="Bibliographies"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtEndPr/><w:sdtContent>
<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> BIBLIOGRAPHY </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Huber, A. (2024). Bodenkunde. Wien: Verlag.</w:t></w:r></w:p>
<w:p><w:r><w:t>Maier, B. (2023). Chemie im Alltag. Graz: Verlag.</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>
</w:sdtContent></w:sdt>
<w:p><w:pPr><w:framePr w:w="4000" w:wrap="around" w:hAnchor="page" w:x="2000" w:y="9000"/></w:pPr><w:r><w:t>Text im Rahmen, Zeile 1</w:t></w:r></w:p>
<w:p><w:pPr><w:framePr w:w="4000" w:wrap="around" w:hAnchor="page" w:x="2000" w:y="9000"/></w:pPr><w:r><w:t>Text im Rahmen, Zeile 2</w:t></w:r></w:p>
<w:p><w:r><w:t>Ende.</w:t></w:r></w:p>
<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1417" w:right="1417" w:bottom="1134" w:left="1417" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr>
</w:body></w:document>"#;

    pub(crate) fn rewrite_zip_for_tests(docx: &[u8], replaced: &HashMap<String, Vec<u8>>) -> Vec<u8> {
        rewrite_zip(docx, replaced).unwrap()
    }

    pub(crate) fn fixture() -> Vec<u8> {
        fixture_doc(DOC)
    }

    /// Wie `fixture`, mit einer Formel E=mc² im Text.
    pub(crate) fn fixture_with_formula() -> Vec<u8> {
        let formula = r#"<w:p><w:r><w:t xml:space="preserve">Es gilt </w:t></w:r><m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><m:r><m:t>E=m</m:t></m:r><m:sSup><m:e><m:r><m:t>c</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath><w:r><w:t>.</w:t></w:r></w:p>"#;
        fixture_doc(&DOC.replacen("<w:p><w:r><w:t>Ende.</w:t></w:r></w:p>", &format!("{formula}<w:p><w:r><w:t>Ende.</w:t></w:r></w:p>"), 1))
    }

    fn fixture_doc(doc: &str) -> Vec<u8> {
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default();
        let parts: [(&str, &str); 5] = [
            ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#),
            ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#),
            ("word/_rels/document.xml.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#),
            ("word/document.xml", doc),
            ("word/styles.xml", STYLES),
        ];
        for (n, c) in parts {
            out.start_file(n, o).unwrap();
            out.write_all(c.as_bytes()).unwrap();
        }
        out.finish().unwrap().into_inner()
    }

    #[test]
    fn complex_link_fields_in_footer_become_text() {
        // so schreibt LibreOffice STYLEREF beim Umwandeln aus ODT
        let xml = r#"<w:ftr xmlns:w="w"><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> STYLEREF &quot;Titel&quot; </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Abschlussbericht</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> – Seite </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>3</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:ftr>"#;
        let (new, n) = unlink_complex_fields(xml.as_bytes(), &field_is_link).unwrap();
        let s = String::from_utf8(new).unwrap();
        assert_eq!(n, 1);
        assert!(!s.contains("STYLEREF"));
        assert!(s.contains("Abschlussbericht"));
        // das Seitenzahl-Feld bleibt ein Feld
        assert!(s.contains(" PAGE ") && s.matches("w:fldCharType=").count() == 3);
    }

    #[test]
    fn std_style_renamed_without_custom_flag() {
        let xml = r#"<w:styles xmlns:w="w"><w:style w:type="paragraph" w:styleId="TD"><w:name w:val="TD"/></w:style></w:styles>"#;
        let (new, ren) = rename_std_styles(xml.as_bytes()).unwrap();
        assert_eq!(ren.len(), 1);
        assert!(String::from_utf8(new).unwrap().contains(r#"<w:name w:val="TD Vorlage"/>"#));
    }

    #[test]
    fn caption_style_renamed_only_in_style() {
        let (new, done) = rename_caption_style(STYLES.as_bytes()).unwrap();
        let s = String::from_utf8(new).unwrap();
        assert!(done);
        assert!(s.contains(&format!(r#"w:styleId="Beschriftung"><w:name w:val="{CAPTION_NEW_NAME}"/>"#)));
        // latentStyles bleiben unverändert
        assert!(s.contains(r#"<w:lsdException w:name="caption""#));
    }

    #[test]
    fn bibliography_becomes_text() {
        let (new, n) = unlink_bibliography(DOC.as_bytes()).unwrap();
        let s = String::from_utf8(new).unwrap();
        assert_eq!(n, 1);
        assert!(!s.contains("BIBLIOGRAPHY"));
        assert!(!s.contains("w:sdt"));
        assert!(!s.contains("Bibliographies"));
        assert!(s.contains("Huber, A. (2024)") && s.contains("Maier, B. (2023)"));
        // das SEQ-Feld der Beschriftung bleibt erhalten
        assert!(s.contains("SEQ Tabelle"));
        assert_eq!(s.matches("w:fldCharType=").count(), 3);
        // Ergebnis ist wohlgeformt
        let mut r = reader(s.as_bytes());
        let mut buf = Vec::new();
        loop {
            if matches!(r.read_event_into(&mut buf).unwrap(), Event::Eof) { break; }
            buf.clear();
        }
    }

    #[test]
    fn other_fields_untouched() {
        let xml = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> TOC \o "1-3" \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>x</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
        let (new, n) = unlink_bibliography(xml.as_bytes()).unwrap();
        assert_eq!(n, 0);
        assert_eq!(String::from_utf8(new).unwrap(), xml);
        assert!(is_bibliography_instr(" ADDIN ZOTERO_BIBL {} CSL_BIBLIOGRAPHY "));
        assert!(is_bibliography_instr("ADDIN EN.REFLIST"));
        assert!(!is_bibliography_instr("ADDIN ZOTERO_ITEM CSL_CITATION {}"));
    }

    #[test]
    fn table_style_spacing_reaches_cells() {
        let st = r#"<w:styles xmlns:w="w"><w:docDefaults><w:pPrDefault><w:pPr><w:spacing w:after="360" w:line="300" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Standard"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="TD"><w:name w:val="TD"/><w:basedOn w:val="Standard"/><w:pPr><w:spacing w:before="80" w:after="60"/></w:pPr></w:style>
<w:style w:type="table" w:default="1" w:styleId="NormaleTabelle"><w:name w:val="Normal Table"/></w:style>
<w:style w:type="table" w:styleId="Raster"><w:name w:val="Table Grid"/><w:basedOn w:val="NormaleTabelle"/><w:pPr><w:spacing w:before="40" w:after="20" w:line="264" w:lineRule="auto"/></w:pPr><w:tblStylePr w:type="firstRow"><w:pPr><w:spacing w:line="500"/></w:pPr></w:tblStylePr></w:style>
<w:style w:type="table" w:styleId="Haus"><w:name w:val="Haus"/><w:basedOn w:val="Raster"/></w:style></w:styles>"#;
        let doc = r#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>außen</w:t></w:r></w:p>
<w:tbl><w:tblPr><w:tblStyle w:val="Haus"/></w:tblPr><w:tr><w:tc>
<w:p><w:pPr><w:pStyle w:val="TD"/><w:jc w:val="right"/></w:pPr><w:r><w:t>a</w:t></w:r></w:p>
<w:p><w:r><w:t>b</w:t></w:r></w:p>
<w:p><w:pPr><w:spacing w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:t>c</w:t></w:r></w:p>
</w:tc></w:tr></w:tbl></w:body></w:document>"#;
        let (out, n) = apply_table_spacing(doc.as_bytes(), st.as_bytes()).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(n, 3, "{out}");
        // TD legt before/after selbst fest: nur der Zeilenabstand kommt aus der Tabellenvorlage
        assert!(out.contains(r#"<w:pStyle w:val="TD"/><w:spacing w:line="264" w:lineRule="auto"/><w:jc"#), "{out}");
        // Standard-Absatz: alles aus der Tabellenvorlage (Standard zählt nicht)
        assert!(out.contains(r#"<w:p><w:pPr><w:spacing w:after="20" w:before="40" w:line="264" w:lineRule="auto"/></w:pPr><w:r><w:t>b"#), "{out}");
        // direkte Formatierung bleibt, fehlende Werte kommen dazu
        assert!(out.contains(r#"<w:spacing w:line="240" w:lineRule="auto" w:after="20" w:before="40"/>"#), "{out}");
        // außerhalb der Tabelle unverändert
        assert!(out.contains(r#"<w:p><w:r><w:t>außen"#));
    }

    #[test]
    fn header_drawings_become_decorative() {
        let x = r#"<w:hdr xmlns:w="w" xmlns:wp="wp"><w:p><w:r><w:drawing><wp:anchor><wp:docPr id="1" name="Balken" descr="Leerer Platzhalter"/></wp:anchor></w:drawing><w:drawing><wp:inline><wp:docPr id="2" name="Logo"><a:extLst xmlns:a="a"><a:ext uri="{X}"/></a:extLst></wp:docPr></wp:inline></w:drawing></w:r></w:p></w:hdr>"#;
        let (out, n) = decorate_all(x.as_bytes()).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(n, 2, "{out}");
        assert_eq!(out.matches(r#"adec:decorative xmlns:adec="http://schemas.microsoft.com/office/drawing/2017/decorative" val="1""#).count(), 2, "{out}");
        assert_eq!(out.matches(DECORATIVE_ALT).count(), 2);
        assert!(out.contains(r#"<a:ext uri="{X}"/>"#));
        // zweiter Lauf: nichts Neues
        assert_eq!(decorate_all(out.as_bytes()).unwrap().1, 0);
    }

    #[test]
    fn local_links_unlinked() {
        assert!(is_local_target(r"file:///C:\Users\x\Downloads\ohne"));
        assert!(is_local_target(r"C:\Daten\a.pdf"));
        assert!(is_local_target(r"\\server\freigabe\a.pdf"));
        assert!(!is_local_target("https://www.ris.bka.gv.at/"));
        assert!(!is_local_target("mailto:a@b.at"));
        let ids: std::collections::HashSet<String> = ["rId9".to_string()].into();
        let doc = r#"<w:p xmlns:w="w" xmlns:r="r"><w:hyperlink r:id="rId9"><w:r><w:t>Abbildung 16 Text 12</w:t></w:r></w:hyperlink><w:hyperlink r:id="rId1"><w:r><w:t>Web</w:t></w:r></w:hyperlink></w:p>"#;
        assert_eq!(local_link_texts(doc.as_bytes(), &ids).unwrap(), vec!["Abbildung 16 Text".to_string()]);
        let (out, n) = unlink_local(doc.as_bytes(), &ids).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(n, 1);
        assert_eq!(out.matches("<w:hyperlink").count(), 1);
        assert!(out.contains("Abbildung 16 Text 12"));
    }

    #[test]
    fn vector_text_in_emf_and_wmf() {
        fn rec(t: u32, body: &[u8]) -> Vec<u8> {
            let mut v = t.to_le_bytes().to_vec();
            v.extend(((8 + body.len()) as u32).to_le_bytes());
            v.extend(body);
            v
        }
        let mut hdr = vec![0u8; 80];
        hdr[40..44].copy_from_slice(b" EMF");
        let mut emf = rec(1, &hdr[8..]);
        emf.extend(rec(84, &[0; 8]));
        emf.extend(rec(84, &[0; 8]));
        emf.extend(rec(14, &[0; 12]));
        assert_eq!(vector_text_count(&emf), 2);
        // WMF: Kopf (18 Bytes), META_TEXTOUT, META_EOF
        let mut wmf = vec![1, 0, 9, 0, 0, 3];
        wmf.extend([0u8; 12]);
        wmf.extend(4u32.to_le_bytes());
        wmf.extend(0x0521u16.to_le_bytes());
        wmf.extend([0u8; 2]);
        wmf.extend(3u32.to_le_bytes());
        wmf.extend(0u16.to_le_bytes());
        assert_eq!(vector_text_count(&wmf), 1);
        assert_eq!(vector_text_count(b"\x89PNG\r\n\x1a\n"), 0);
    }

    /// Ende-zu-Ende mit LibreOffice: `cargo test -- --ignored`.
    /// Schreibt Original und vorbereitete Kopie nach MIMI_TEST_OUT (Standard: target/prep-test).
    #[test]
    #[ignore]
    fn write_fixture_for_libreoffice() {
        let dir = std::path::PathBuf::from(std::env::var("MIMI_TEST_OUT").unwrap_or("target/prep-test".into()));
        std::fs::create_dir_all(&dir).unwrap();
        // MIMI_TEST_IN=bericht.docx: echtes Dokument statt des eingebauten Beispiels
        let input = std::env::var("MIMI_TEST_IN").ok();
        let orig = match &input {
            Some(p) => std::fs::read(p).unwrap(),
            None => fixture(),
        };
        // MIMI_TEST_APPLY=1: vorher alles anwenden, was man in der Oberfläche anklicken
        // kann (Alternativtexte als Platzhalter, Linkbeschreibungen, Ebenen, Farben, Tabellenköpfe)
        let orig = if std::env::var("MIMI_TEST_APPLY").is_ok() {
            let a = analyze(&orig).unwrap();
            let ch: Changes = serde_json::from_value(serde_json::json!({
                "title": if a.title.is_empty() { "Testtitel".to_string() } else { a.title.clone() },
                "language": if a.language.is_empty() { "de-AT".to_string() } else { a.language.clone() },
                "images": a.images.iter().map(|i| serde_json::json!({"index": i.index, "alt": format!("Platzhalter {}", i.index), "decorative": false})).collect::<Vec<_>>(),
                "tables": a.tables.iter().map(|t| serde_json::json!({"index": t.index, "header": true})).collect::<Vec<_>>(),
                "link_descriptions": true,
                "headings": a.heading_fixes.iter().map(|f| serde_json::json!({"para": f.para, "style": f.style})).collect::<Vec<_>>(),
                "colors": a.colors.iter().map(|c| serde_json::json!({"from": c.color, "to": c.suggestion})).collect::<Vec<_>>(),
            }))
            .unwrap();
            println!("angewendet: {} Bilder, {} Tabellen, {} Ebenen, {} Farben, {} Links ohne Beschreibung", a.images.len(), a.tables.len(), a.heading_fixes.len(), a.colors.len(), a.links_missing);
            apply(&orig, &ch).unwrap()
        } else {
            orig
        };
        let (prep, notes) = prepare_for_pdf(&orig).unwrap();
        std::fs::write(dir.join("original.docx"), &orig).unwrap();
        std::fs::write(dir.join("vorbereitet.docx"), &prep).unwrap();
        for n in &notes {
            println!("{n}");
        }
        if input.is_none() {
            assert_eq!(notes.len(), 3, "{notes:?}");
        }
        for h in analyze(&orig).unwrap().hints.iter().filter(|h| matches!(h.kind.as_str(), "grafiktext" | "link")) {
            println!("[{}] {}", h.severity, h.text);
        }
    }
}
