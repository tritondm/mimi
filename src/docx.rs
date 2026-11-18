// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 example.org

//! Analyse und Korrektur von Word-Dateien (DOCX) im Hinblick auf Barrierefreiheit.
//!
//! Die XML-Teile werden ereignisbasiert gelesen und unverändert durchgereicht;
//! nur die gezielt geänderten Elemente werden neu geschrieben. Dadurch bleibt
//! der Rest des Dokuments Byte für Byte erhalten.

use anyhow::{Context, Result};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

const DECORATIVE_URI: &str = "{C183D7F6-B498-43B3-948B-1728B52AA6E4}";
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_ADEC: &str = "http://schemas.microsoft.com/office/drawing/2017/decorative";

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
}

#[derive(Serialize, Clone)]
pub struct Hint {
    /// "fehler", "warnung", "hinweis"
    pub severity: String,
    pub kind: String,
    pub text: String,
    pub section: String,
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
}

#[derive(Deserialize, Default)]
pub struct Changes {
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
    let mut zip = zip::ZipArchive::new(Cursor::new(zip_bytes)).context("Keine gültige DOCX-Datei")?;
    let result = match zip.by_name(name) {
        Ok(mut f) => {
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            Some(buf)
        }
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
    let doc = read_entry(docx, "word/document.xml")?.context("word/document.xml fehlt – ist das eine Word-Datei?")?;
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
                                    text: format!("{} leere Absätze hintereinander (nach „{}“). Abstände besser über die Formatvorlage einstellen.", empty_streak, snippet(&empty_after, 60)),
                                    section: section.clone(),
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
                                    hints.push(Hint {
                                        severity: "warnung".into(),
                                        kind: "ebene".into(),
                                        text: format!("Überschrift Ebene {} folgt direkt auf Ebene {}: „{}“", lvl, last_level, snippet(&text, 80)),
                                        section: section.clone(),
                                    });
                                }
                                if last_level == 0 && lvl > 1 {
                                    hints.push(Hint {
                                        severity: "hinweis".into(),
                                        kind: "ebene".into(),
                                        text: format!("Erste Überschrift hat Ebene {} statt 1: „{}“", lvl, snippet(&text, 80)),
                                        section: section.clone(),
                                    });
                                }
                                last_level = lvl;
                                headings.push(HeadingInfo { level: lvl, text: snippet(&text, 120) });
                                section = snippet(&text, 120);
                            }
                        } else if !p.in_table && !styles.is_caption(&style) && !styles.is_toc_or_index(&style) {
                            let len = text.chars().count();
                            if p.text_runs > 0 && p.bold_runs == p.text_runs && (3..=100).contains(&len) && !text.ends_with('.') && !text.ends_with(':') {
                                hints.push(Hint {
                                    severity: "warnung".into(),
                                    kind: "scheinueberschrift".into(),
                                    text: format!("Fett gesetzter Absatz sieht aus wie eine Überschrift, ist aber keine: „{}“. Falls es eine Überschrift ist, Formatvorlage „Überschrift“ verwenden.", snippet(&text, 80)),
                                    section: section.clone(),
                                });
                            }
                            let manual_list = ["- ", "• ", "– ", "* ", "▪ ", "· ", "o\t", "-\t", "•\t"].iter().any(|m| p.text.trim_start().starts_with(m));
                            if manual_list && !p.has_num {
                                hints.push(Hint {
                                    severity: "warnung".into(),
                                    kind: "liste".into(),
                                    text: format!("Aufzählung von Hand getippt: „{}“. Besser die Aufzählungs-Funktion bzw. -Formatvorlage verwenden.", snippet(&text, 80)),
                                    section: section.clone(),
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

    // Zusammenfassende Hinweise
    if title.is_empty() {
        hints.insert(0, Hint { severity: "fehler".into(), kind: "titel".into(), text: "Kein Dokumenttitel gesetzt (Datei → Eigenschaften → Titel).".into(), section: String::new() });
    }
    if styles.language.is_empty() {
        hints.insert(0, Hint { severity: "fehler".into(), kind: "sprache".into(), text: "Keine Dokumentsprache festgelegt.".into(), section: String::new() });
    }
    if headings.is_empty() {
        hints.insert(0, Hint { severity: "fehler".into(), kind: "ebene".into(), text: "Das Dokument enthält keine Überschriften mit Überschrift-Formatvorlagen. Ohne sie gibt es keine Gliederung und keine Lesezeichen im PDF.".into(), section: String::new() });
    }
    if url_links > 0 {
        hints.push(Hint { severity: "hinweis".into(), kind: "link".into(), text: format!("{} Link(s) zeigen die nackte Webadresse als Text. Ein sprechender Linktext ist besser verständlich (für PAC kein Fehler).", url_links), section: String::new() });
    }
    for t in tables.iter().filter(|t| t.rows <= 1) {
        hints.push(Hint { severity: "hinweis".into(), kind: "tabelle".into(), text: format!("Tabelle {} hat nur eine Zeile – wird sie nur fürs Layout verwendet? Dann besser ohne Tabelle gestalten.", t.index + 1), section: t.section.clone() });
    }

    let links = hyperlink_texts(&doc)?;
    let links_missing = links.iter().filter(|(has, t)| !has && !t.is_empty()).count();
    Ok(Analysis { title, language: styles.language, images, tables, headings, hints, links_total: links.len(), links_missing })
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

/// Wendet die Änderungen an und liefert die neue DOCX-Datei.
pub fn apply(docx: &[u8], changes: &Changes) -> Result<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(docx))?;
    let mut replaced: HashMap<String, Vec<u8>> = HashMap::new();

    if !changes.images.is_empty() || !changes.tables.is_empty() || changes.link_descriptions {
        let doc = read_entry(docx, "word/document.xml")?.context("word/document.xml fehlt")?;
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
    Ok(out.finish()?.into_inner())
}
