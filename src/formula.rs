// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 example.org

//! Formeln aus dem Formeleditor von Word (OMML, `m:oMath`).
//!
//! PDF/UA-1 verlangt für jede Formel einen Alternativtext (Matterhorn 17, veraPDF 7.7-1).
//! LibreOffice taggt Word-Formeln als `Formula`, aber ohne `Alt`, und Word selbst hat für
//! Formeln kein Feld für einen Alternativtext. Mimi
//!
//! 1. findet die Formeln (Text, Fußnoten, Endnoten – in dieser Reihenfolge, so wie sie im
//!    Strukturbaum des PDFs stehen),
//! 2. schlägt die **Linearform** vor (wie Word sie unter „Linear“ zeigt, z. B. `E=mc^2`),
//! 3. speichert einen eigenen Text als benutzerdefinierte Dokumenteigenschaft
//!    `Mimi-Formel-<Prüfsumme>` (bleibt in der Word-Datei erhalten; die Prüfsumme kommt aus
//!    der Linearform, eine geänderte Formel wird also neu abgefragt),
//! 4. schreibt beim Export den Text als `Alt` an die `Formula`-Elemente (siehe `pdffix`).

use anyhow::Result;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::docx::read_entry;

const PROP_PREFIX: &str = "Mimi-Formel-";
/// frühere Namen der Eigenschaft (vor der Umbenennung in Mimi) – werden weiter gelesen
const LEGACY_PREFIXES: &[&str] = &["Leo-Formel-"];

/// Schlüssel aus dem Namen einer Eigenschaft (neuer oder früherer Name), sonst `None`.
fn prop_key(name: &str) -> Option<&str> {
    std::iter::once(PROP_PREFIX).chain(LEGACY_PREFIXES.iter().copied()).find_map(|p| name.strip_prefix(p))
}
const NS_CUSTOM: &str = "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties";
const NS_VT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes";
const FMTID: &str = "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}";
const CT_CUSTOM: &str = "application/vnd.openxmlformats-officedocument.custom-properties+xml";
const REL_CUSTOM: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties";

/// Teile der Word-Datei, deren Formeln im PDF getaggt werden – in der Reihenfolge des Strukturbaums.
/// Kopf- und Fußzeilen fehlen absichtlich: sie werden im PDF zu Artefakten.
const PARTS: [(&str, &str); 3] = [("word/document.xml", "Text"), ("word/footnotes.xml", "Fußnote"), ("word/endnotes.xml", "Endnote")];

#[derive(Serialize, Clone, Debug)]
pub struct FormulaInfo {
    pub index: usize,
    /// Schlüssel der Dokumenteigenschaft (ohne Präfix)
    pub key: String,
    /// Linearform, z. B. "E=mc^2"
    pub linear: String,
    /// eigener Alternativtext (leer = Linearform verwenden)
    pub alt: String,
    /// Text des Absatzes, die Formel als „[Formel]“
    pub context: String,
    /// "Text", "Fußnote", "Endnote"
    pub place: String,
}

#[derive(Deserialize)]
pub struct FormulaChange {
    pub key: String,
    /// leer = eigenen Text entfernen, Linearform verwenden
    pub alt: String,
}

// ---------------------------------------------------------------------------
// OMML als kleiner Baum
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    kids: Vec<Kid>,
}

#[derive(Debug)]
enum Kid {
    Node(Node),
    Text(String),
}

impl Node {
    fn child(&self, name: &str) -> Option<&Node> {
        self.kids.iter().find_map(|k| match k {
            Kid::Node(n) if n.name == name => Some(n),
            _ => None,
        })
    }
    fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.kids.iter().filter_map(move |k| match k {
            Kid::Node(n) if n.name == name => Some(n),
            _ => None,
        })
    }
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    /// Wert von z. B. `m:dPr/m:begChr/@m:val`
    fn prop(&self, pr: &str, key: &str) -> Option<&str> {
        self.child(pr)?.child(key)?.attr("m:val")
    }
}

fn start_node(e: &quick_xml::events::BytesStart) -> Node {
    let attrs = e
        .attributes()
        .flatten()
        .filter_map(|a| Some((String::from_utf8_lossy(a.key.as_ref()).into_owned(), a.unescape_value().ok()?.into_owned())))
        .collect();
    Node { name: String::from_utf8_lossy(e.name().as_ref()).into_owned(), attrs, kids: Vec::new() }
}

/// Liest den Teilbaum ab einem bereits gelesenen Start-Element bis zu seinem Ende.
fn read_subtree(r: &mut Reader<&[u8]>, root: Node) -> Result<Node> {
    let mut stack = vec![root];
    let mut buf = Vec::new();
    loop {
        match r.read_event_into(&mut buf)? {
            Event::Start(e) => stack.push(start_node(&e)),
            Event::Empty(e) => {
                let n = start_node(&e);
                stack.last_mut().unwrap().kids.push(Kid::Node(n));
            }
            Event::Text(t) => {
                let s = t.unescape().map(|s| s.into_owned()).unwrap_or_default();
                stack.last_mut().unwrap().kids.push(Kid::Text(s));
            }
            Event::End(_) => {
                let n = stack.pop().unwrap();
                match stack.last_mut() {
                    Some(p) => p.kids.push(Kid::Node(n)),
                    None => return Ok(n),
                }
            }
            Event::Eof => anyhow::bail!("Formel nicht abgeschlossen"),
            _ => {}
        }
        buf.clear();
    }
}

// ---------------------------------------------------------------------------
// Linearform
// ---------------------------------------------------------------------------

/// Einzelnes Zeichen oder einfache Zahl/Name bleibt, alles andere kommt in Klammern.
fn atom(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= 1 || s.chars().all(|c| c.is_alphanumeric() || c == ',' || c == '.') || (s.starts_with('(') && s.ends_with(')') && balanced_inside(s)) {
        s.to_string()
    } else {
        format!("({s})")
    }
}

/// „(a+b)“ ist schon geklammert, „(a)+(b)“ nicht.
fn balanced_inside(s: &str) -> bool {
    let inner = &s[1..s.len() - 1];
    let mut depth = 0i32;
    for c in inner.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn part(n: &Node, name: &str) -> String {
    n.child(name).map(lin).unwrap_or_default()
}

fn lin(n: &Node) -> String {
    match n.name.as_str() {
        "m:t" => n.kids.iter().map(|k| if let Kid::Text(t) = k { t.as_str() } else { "" }).collect(),
        "m:sSup" => format!("{}^{}", atom(&part(n, "m:e")), atom(&part(n, "m:sup"))),
        "m:sSub" => format!("{}_{}", atom(&part(n, "m:e")), atom(&part(n, "m:sub"))),
        "m:sSubSup" => format!("{}_{}^{}", atom(&part(n, "m:e")), atom(&part(n, "m:sub")), atom(&part(n, "m:sup"))),
        "m:sPre" => format!("_{}^{} {}", atom(&part(n, "m:sub")), atom(&part(n, "m:sup")), atom(&part(n, "m:e"))),
        "m:f" => format!("{}/{}", atom(&part(n, "m:num")), atom(&part(n, "m:den"))),
        "m:rad" => {
            let deg = part(n, "m:deg");
            let e = atom(&part(n, "m:e"));
            match deg.trim() {
                "" => format!("√{e}"),
                "3" => format!("∛{e}"),
                "4" => format!("∜{e}"),
                d => format!("√[{d}]{e}"),
            }
        }
        "m:nary" => {
            let chr = n.prop("m:naryPr", "m:chr").unwrap_or("∫").to_string();
            let mut s = chr;
            let sub = part(n, "m:sub");
            let sup = part(n, "m:sup");
            if !sub.trim().is_empty() {
                s += &format!("_{}", atom(&sub));
            }
            if !sup.trim().is_empty() {
                s += &format!("^{}", atom(&sup));
            }
            format!("{s} {}", part(n, "m:e").trim())
        }
        "m:d" => {
            let beg = n.prop("m:dPr", "m:begChr").unwrap_or("(");
            let end = n.prop("m:dPr", "m:endChr").unwrap_or(")");
            let sep = n.prop("m:dPr", "m:sepChr").unwrap_or("|");
            let items: Vec<String> = n.children("m:e").map(|e| lin(e).trim().to_string()).collect();
            format!("{beg}{}{end}", items.join(sep))
        }
        "m:func" => {
            let name = part(n, "m:fName");
            let arg = atom(&part(n, "m:e"));
            if arg.starts_with('(') {
                format!("{}{arg}", name.trim())
            } else {
                format!("{} {arg}", name.trim())
            }
        }
        "m:limLow" => format!("{}_{}", part(n, "m:e").trim(), atom(&part(n, "m:lim"))),
        "m:limUpp" => format!("{}^{}", part(n, "m:e").trim(), atom(&part(n, "m:lim"))),
        "m:acc" => format!("{}{}", part(n, "m:e"), n.prop("m:accPr", "m:chr").unwrap_or("\u{302}")),
        "m:eqArr" => n.children("m:e").map(|e| lin(e).trim().to_string()).collect::<Vec<_>>().join("; "),
        "m:m" => {
            let rows: Vec<String> = n.children("m:mr").map(|r| r.children("m:e").map(|e| lin(e).trim().to_string()).collect::<Vec<_>>().join(", ")).collect();
            format!("[{}]", rows.join("; "))
        }
        "m:oMathPara" => n.children("m:oMath").map(|e| lin(e).trim().to_string()).collect::<Vec<_>>().join(" "),
        // Formatierung und Eigenschaften gehören nicht zum Inhalt
        name if name.ends_with("Pr") || name.starts_with("w:") => String::new(),
        // m:oMath, m:r, m:e, m:num, m:box, m:borderBox, m:bar, m:groupChr, m:phant …: Inhalt
        _ => n.kids.iter().map(|k| if let Kid::Node(c) = k { lin(c) } else { String::new() }).collect(),
    }
}

/// Linearform einer Formel, Leerraum zusammengefasst.
fn linear(n: &Node) -> String {
    lin(n).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn key_of(linear: &str) -> String {
    // FNV-1a, 64 Bit – stabil über Programmversionen
    let mut h: u64 = 0xcbf29ce484222325;
    for b in linear.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

// ---------------------------------------------------------------------------
// Formeln finden
// ---------------------------------------------------------------------------

/// Platzhalter für eine Formel im Absatztext
const MARK: char = '\u{FFFC}';

fn scan_part(xml: &[u8], place: &str, out: &mut Vec<FormulaInfo>) -> Result<()> {
    let mut r = Reader::from_reader(xml);
    r.config_mut().trim_text(false);
    let mut buf = Vec::new();
    // offene Absätze: (Text, Indizes der Formeln darin)
    let mut paras: Vec<(String, Vec<usize>)> = Vec::new();
    let mut in_text = false;
    loop {
        match r.read_event_into(&mut buf)? {
            Event::Start(e) => match e.name().as_ref() {
                // Textfelder stehen bei Word doppelt drin (neu + VML-Ersatz) – nur einmal zählen
                b"mc:Fallback" => {
                    let end = e.to_end().into_owned();
                    r.read_to_end_into(end.name(), &mut Vec::new())?;
                }
                b"w:p" => paras.push((String::new(), Vec::new())),
                b"w:t" => in_text = true,
                // jede m:oMath einzeln (auch mehrere in einem Formelabsatz m:oMathPara)
                b"m:oMath" => {
                    let node = read_subtree(&mut r, start_node(&e))?;
                    let lin = linear(&node);
                    if !lin.is_empty() {
                        let index = out.len();
                        out.push(FormulaInfo { index, key: key_of(&lin), linear: lin, alt: String::new(), context: String::new(), place: place.into() });
                        if let Some(p) = paras.last_mut() {
                            p.0.push(MARK);
                            p.1.push(index);
                        }
                    }
                }
                _ => {}
            },
            Event::Text(t) if in_text => {
                if let Some(p) = paras.last_mut() {
                    p.0.push_str(&t.unescape().unwrap_or_default());
                }
            }
            Event::End(e) => match e.name().as_ref() {
                b"w:t" => in_text = false,
                b"w:p" => {
                    if let Some((text, idx)) = paras.pop() {
                        let ctx = text.replace(MARK, " [Formel] ").split_whitespace().collect::<Vec<_>>().join(" ");
                        let ctx = if ctx.chars().count() > 160 { format!("{}…", ctx.chars().take(160).collect::<String>()) } else { ctx };
                        for i in idx {
                            out[i].context = ctx.clone();
                        }
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(())
}

/// Eigene Alternativtexte aus `docProps/custom.xml`: Schlüssel → Text.
fn stored_alts(docx: &[u8]) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    let Some(xml) = read_entry(docx, "docProps/custom.xml")? else { return Ok(map) };
    let mut r = Reader::from_reader(xml.as_slice());
    let mut buf = Vec::new();
    let mut cur: Option<String> = None;
    let mut in_val = false;
    loop {
        match r.read_event_into(&mut buf)? {
            Event::Start(e) if e.local_name().as_ref() == b"property" => {
                cur = e.attributes().flatten().find(|a| a.key.as_ref() == b"name").and_then(|a| a.unescape_value().ok()).and_then(|n| prop_key(&n).map(str::to_string));
            }
            Event::Start(e) if cur.is_some() && e.local_name().as_ref() == b"lpwstr" => in_val = true,
            Event::Text(t) if in_val => {
                if let Some(k) = &cur {
                    map.insert(k.clone(), t.unescape().unwrap_or_default().into_owned());
                }
            }
            Event::End(e) if e.local_name().as_ref() == b"lpwstr" => in_val = false,
            Event::End(e) if e.local_name().as_ref() == b"property" => cur = None,
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(map)
}

/// Alle Formeln der Word-Datei, mit gespeichertem eigenem Text.
pub fn find(docx: &[u8]) -> Result<Vec<FormulaInfo>> {
    let mut out = Vec::new();
    for (name, place) in PARTS {
        if let Some(xml) = read_entry(docx, name)? {
            scan_part(&xml, place, &mut out)?;
        }
    }
    if !out.is_empty() {
        let stored = stored_alts(docx)?;
        for f in &mut out {
            if let Some(a) = stored.get(&f.key) {
                f.alt = a.clone();
            }
        }
    }
    Ok(out)
}

/// Alternativtexte für das PDF, in der Reihenfolge des Strukturbaums.
pub fn pdf_alts(docx: &[u8]) -> Result<Vec<String>> {
    Ok(find(docx)?.into_iter().map(|f| if f.alt.trim().is_empty() { f.linear } else { f.alt }).collect())
}

// ---------------------------------------------------------------------------
// Eigene Texte speichern (docProps/custom.xml)
// ---------------------------------------------------------------------------

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Vorhandene Eigenschaften, die nicht von Mimi stammen, roh übernehmen; höchste `pid`.
fn foreign_properties(xml: &str) -> (Vec<String>, u32) {
    let mut keep = Vec::new();
    let mut max_pid = 1;
    let mut rest = xml;
    while let Some(a) = rest.find("<property") {
        let tail = &rest[a..];
        let Some(b) = tail.find("</property>") else { break };
        let chunk = &tail[..b + "</property>".len()];
        if let Some(p) = chunk.find("pid=\"") {
            if let Some(n) = chunk[p + 5..].split('"').next().and_then(|v| v.parse::<u32>().ok()) {
                max_pid = max_pid.max(n);
            }
        }
        // eigene Einträge (auch unter früherem Namen) werden neu geschrieben
        let ours = std::iter::once(PROP_PREFIX).chain(LEGACY_PREFIXES.iter().copied()).any(|p| chunk.contains(&format!("name=\"{p}")));
        if !ours {
            keep.push(chunk.to_string());
        }
        rest = &tail[b + "</property>".len()..];
    }
    (keep, max_pid)
}

/// Schreibt die Änderungen in `replaced` (neue oder geänderte ZIP-Einträge).
pub fn apply_alts(docx: &[u8], changes: &[FormulaChange], replaced: &mut HashMap<String, Vec<u8>>) -> Result<()> {
    if changes.is_empty() {
        return Ok(());
    }
    let formulas = find(docx)?;
    let linear: HashMap<&str, &str> = formulas.iter().map(|f| (f.key.as_str(), f.linear.as_str())).collect();
    let mut alts = stored_alts(docx)?;
    // Texte zu Formeln, die es nicht mehr gibt, aufräumen
    alts.retain(|k, _| linear.contains_key(k.as_str()));
    for c in changes {
        let Some(lin) = linear.get(c.key.as_str()) else { continue };
        let a = c.alt.trim();
        if a.is_empty() || a == *lin {
            alts.remove(&c.key);
        } else {
            alts.insert(c.key.clone(), a.to_string());
        }
    }

    let existing = read_entry(docx, "docProps/custom.xml")?.map(|b| String::from_utf8_lossy(&b).into_owned());
    let (mut props, mut pid) = existing.as_deref().map(foreign_properties).unwrap_or((Vec::new(), 1));
    let mut keys: Vec<&String> = alts.keys().collect();
    keys.sort();
    for k in keys {
        pid += 1;
        props.push(format!(r#"<property fmtid="{FMTID}" pid="{pid}" name="{PROP_PREFIX}{k}"><vt:lpwstr>{}</vt:lpwstr></property>"#, xml_escape(&alts[k])));
    }
    let custom = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<Properties xmlns="{NS_CUSTOM}" xmlns:vt="{NS_VT}">{}</Properties>"#, "\n", props.concat());
    replaced.insert("docProps/custom.xml".into(), custom.into_bytes());

    if existing.is_none() {
        // neuer Teil: Inhaltstyp und Verweis im Paket eintragen
        if let Some(ct) = read_entry(docx, "[Content_Types].xml")? {
            let s = String::from_utf8_lossy(&ct);
            if !s.contains("/docProps/custom.xml") {
                let s = s.replacen("</Types>", &format!(r#"<Override PartName="/docProps/custom.xml" ContentType="{CT_CUSTOM}"/></Types>"#), 1);
                replaced.insert("[Content_Types].xml".into(), s.into_bytes());
            }
        }
        if let Some(rels) = read_entry(docx, "_rels/.rels")? {
            let s = String::from_utf8_lossy(&rels);
            if !s.contains(REL_CUSTOM) {
                let s = s.replacen("</Relationships>", &format!(r#"<Relationship Id="rIdLeoCustom" Type="{REL_CUSTOM}" Target="docProps/custom.xml"/></Relationships>"#), 1);
                replaced.insert("_rels/.rels".into(), s.into_bytes());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lin_of(omml: &str) -> String {
        let xml = format!(r#"<w:p xmlns:w="w" xmlns:m="m"><w:r><w:t>Vor </w:t></w:r>{omml}<w:r><w:t> nach.</w:t></w:r></w:p>"#);
        let mut out = Vec::new();
        scan_part(xml.as_bytes(), "Text", &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].context, "Vor [Formel] nach.");
        out.remove(0).linear
    }

    #[test]
    fn linear_forms() {
        let r = |t: &str| format!("<m:r><m:t>{t}</m:t></m:r>");
        // E=mc²
        assert_eq!(lin_of(&format!("<m:oMath>{}{}{}<m:sSup><m:e>{}</m:e><m:sup>{}</m:sup></m:sSup></m:oMath>", r("E"), r("="), r("m"), r("c"), r("2"))), "E=mc^2");
        // Bruch mit zusammengesetztem Zähler
        assert_eq!(lin_of(&format!("<m:oMath><m:f><m:num>{}</m:num><m:den>{}</m:den></m:f></m:oMath>", r("a+b"), r("2"))), "(a+b)/2");
        // Wurzel, Klammer, Summe
        assert_eq!(lin_of(&format!("<m:oMath><m:rad><m:radPr><m:degHide m:val=\"1\"/></m:radPr><m:deg/><m:e>{}</m:e></m:rad></m:oMath>", r("x"))), "√x");
        assert_eq!(lin_of(&format!("<m:oMath><m:d><m:e>{}</m:e></m:d></m:oMath>", r("a+b"))), "(a+b)");
        assert_eq!(
            lin_of(&format!("<m:oMath><m:nary><m:naryPr><m:chr m:val=\"∑\"/></m:naryPr><m:sub>{}</m:sub><m:sup>{}</m:sup><m:e>{}</m:e></m:nary></m:oMath>", r("i=1"), r("n"), r("i"))),
            "∑_(i=1)^n i"
        );
        // Formatierung gehört nicht dazu
        assert_eq!(lin_of("<m:oMath><m:r><w:rPr><w:rFonts w:ascii=\"Cambria Math\"/></w:rPr><m:t>x</m:t></m:r></m:oMath>"), "x");
    }

    #[test]
    fn fallback_counted_once() {
        let xml = r#"<w:body xmlns:w="w" xmlns:m="m" xmlns:mc="mc"><mc:AlternateContent><mc:Choice><w:p><m:oMath><m:r><m:t>x</m:t></m:r></m:oMath></w:p></mc:Choice><mc:Fallback><w:p><m:oMath><m:r><m:t>x</m:t></m:r></m:oMath></w:p></mc:Fallback></mc:AlternateContent></w:body>"#;
        let mut out = Vec::new();
        scan_part(xml.as_bytes(), "Text", &mut out).unwrap();
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn store_and_read_alt() {
        let docx = crate::docx::tests::fixture_with_formula();
        let f = find(&docx).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].linear, "E=mc^2");
        assert!(f[0].alt.is_empty());

        // eigenen Text speichern – neuer Teil custom.xml samt Inhaltstyp und Verweis
        let changes = [FormulaChange { key: f[0].key.clone(), alt: "E gleich m mal c Quadrat".into() }];
        let docx2 = crate::docx::apply(&docx, &crate::docx::Changes { formulas: changes.into(), ..Default::default() }).unwrap();
        let f2 = find(&docx2).unwrap();
        assert_eq!(f2[0].alt, "E gleich m mal c Quadrat");
        assert_eq!(pdf_alts(&docx2).unwrap(), vec!["E gleich m mal c Quadrat".to_string()]);
        let ct = String::from_utf8(read_entry(&docx2, "[Content_Types].xml").unwrap().unwrap()).unwrap();
        assert!(ct.contains("/docProps/custom.xml"));
        let rels = String::from_utf8(read_entry(&docx2, "_rels/.rels").unwrap().unwrap()).unwrap();
        assert!(rels.contains(REL_CUSTOM));

        // zurück auf die Linearform: Eigenschaft verschwindet
        let back = [FormulaChange { key: f[0].key.clone(), alt: "E=mc^2".into() }];
        let docx3 = crate::docx::apply(&docx2, &crate::docx::Changes { formulas: back.into(), ..Default::default() }).unwrap();
        assert!(find(&docx3).unwrap()[0].alt.is_empty());
        let custom = String::from_utf8(read_entry(&docx3, "docProps/custom.xml").unwrap().unwrap()).unwrap();
        assert!(!custom.contains(PROP_PREFIX));
        assert_eq!(pdf_alts(&docx3).unwrap(), vec!["E=mc^2".to_string()]);
    }

    #[test]
    fn reads_legacy_leo_properties() {
        // Datei mit einem Formeltext von vor der Umbenennung (Leo-Formel-…)
        let docx = crate::docx::tests::fixture_with_formula();
        let key = find(&docx).unwrap()[0].key.clone();
        let custom = format!(r#"<?xml version="1.0"?><Properties xmlns="{NS_CUSTOM}" xmlns:vt="{NS_VT}"><property fmtid="{FMTID}" pid="2" name="Leo-Formel-{key}"><vt:lpwstr>alter Text</vt:lpwstr></property></Properties>"#);
        let mut rep = HashMap::new();
        rep.insert("docProps/custom.xml".to_string(), custom.into_bytes());
        let docx = crate::docx::tests::rewrite_zip_for_tests(&docx, &rep);
        assert_eq!(find(&docx).unwrap()[0].alt, "alter Text");
        // beim nächsten Speichern unter neuem Namen, alter Eintrag weg
        let ch = [FormulaChange { key: key.clone(), alt: "neuer Text".into() }];
        let docx2 = crate::docx::apply(&docx, &crate::docx::Changes { formulas: ch.into(), ..Default::default() }).unwrap();
        let c = String::from_utf8(read_entry(&docx2, "docProps/custom.xml").unwrap().unwrap()).unwrap();
        assert!(c.contains(&format!("Mimi-Formel-{key}")) && !c.contains("Leo-Formel-"));
        assert_eq!(find(&docx2).unwrap()[0].alt, "neuer Text");
    }

    #[test]
    fn keeps_foreign_properties() {
        let xml = format!(r#"<Properties xmlns="{NS_CUSTOM}" xmlns:vt="{NS_VT}"><property fmtid="{FMTID}" pid="2" name="Projekt"><vt:lpwstr>Praxistest</vt:lpwstr></property><property fmtid="{FMTID}" pid="7" name="{PROP_PREFIX}abc"><vt:lpwstr>alt</vt:lpwstr></property></Properties>"#);
        let (keep, pid) = foreign_properties(&xml);
        assert_eq!(keep.len(), 1);
        assert!(keep[0].contains("Projekt"));
        assert_eq!(pid, 7);
    }
}
