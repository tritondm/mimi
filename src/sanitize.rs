// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 example.org

//! Schutz vor präparierten Dateien, bevor sie verarbeitet oder an LibreOffice gegeben werden:
//!
//! - **Entpackgrenzen** gegen ZIP-Bomben: eine kleine, stark komprimierte Datei, die entpackt
//!   Gigabytes groß ist und den Dienst über den Speicher abstürzen ließe.
//! - **Externe Inhalte entfernen**: verknüpfte Bilder, Vorlagen, Schriften oder Abschnitte, die
//!   LibreOffice sonst beim Umwandeln aus dem Netz oder vom Dateisystem des Servers nachlädt.
//!   Hyperlinks bleiben erhalten, die ruft LibreOffice nicht ab.

use crate::{i18n, tr};
use anyhow::{Context, Result, bail};
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};

/// Grenzen für das Entpacken einer hochgeladenen Datei.
#[derive(Clone, Copy)]
pub struct Limits {
    /// entpackte Größe eines einzelnen Eintrags
    pub entry: u64,
    /// entpackte Größe aller Einträge zusammen
    pub total: u64,
    /// Zahl der Einträge
    pub entries: usize,
}

/// Reicht für große Berichte (viele Bilder, lange Texte) mit viel Abstand.
pub const LIMITS: Limits = Limits { entry: 256 << 20, total: 1 << 30, entries: 10_000 };

/// Prüft das ZIP-Archiv (DOCX oder ODT), bevor es verarbeitet wird. Gezählt wird beim
/// tatsächlichen Entpacken, ohne die Daten aufzuheben: Die Größenangaben im Verzeichnis des
/// Archivs kann ein Angreifer frei wählen.
pub fn check_zip(bytes: &[u8]) -> Result<()> {
    check_zip_with(bytes, LIMITS)
}

fn check_zip_with(bytes: &[u8], lim: Limits) -> Result<()> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context(i18n::t("Die Datei ist beschädigt oder kein Word-/OpenDocument-Dokument", "The file is damaged or not a Word/OpenDocument document"))?;
    if zip.len() > lim.entries {
        bail!(tr!("Die Datei besteht aus mehr als {} Teilen und wird nicht verarbeitet.", "The file consists of more than {} parts and will not be processed.", lim.entries));
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        let f = zip.by_index(i).context(i18n::t("Ein Teil der Datei lässt sich nicht entpacken", "Part of the file cannot be unpacked"))?;
        let name = f.name().to_string();
        let too_big = || anyhow::anyhow!(tr!("Der Teil „{name}“ ist entpackt größer als {} MB. Die Datei wird nicht verarbeitet.", "The part “{name}” is larger than {} MB when unpacked. The file will not be processed.", lim.entry >> 20));
        if f.size() > lim.entry {
            return Err(too_big());
        }
        let n = std::io::copy(&mut f.take(lim.entry + 1), &mut std::io::sink()).context(i18n::t("Ein Teil der Datei lässt sich nicht entpacken", "Part of the file cannot be unpacked"))?;
        if n > lim.entry {
            return Err(too_big());
        }
        total += n;
        if total > lim.total {
            bail!(tr!("Die Datei ist entpackt größer als {} MB und wird nicht verarbeitet.", "The file is larger than {} MB when unpacked and will not be processed.", lim.total >> 20));
        }
    }
    Ok(())
}

/// Einen Eintrag entpacken, höchstens `LIMITS.entry` Bytes.
pub fn read_limited(f: impl Read) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    f.take(LIMITS.entry + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > LIMITS.entry {
        bail!(tr!("Ein Teil der Datei ist entpackt größer als {} MB.", "Part of the file is larger than {} MB when unpacked.", LIMITS.entry >> 20));
    }
    Ok(buf)
}

fn reader(xml: &[u8]) -> Reader<&[u8]> {
    let mut r = Reader::from_reader(xml);
    r.config_mut().trim_text(false);
    r
}

fn attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.as_ref() == key).and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

// ---------------------------------------------------------------------------
// DOCX: externe Beziehungen
// ---------------------------------------------------------------------------

/// Entfernt aus einer `.rels`-Datei alle externen Beziehungen außer Hyperlinks: verknüpfte
/// Bilder (`r:link`), angehängte Vorlagen, OLE-Objekte, Diagrammdaten, Teildokumente …
fn strip_rels(xml: &[u8]) -> Result<(Vec<u8>, usize)> {
    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(xml.len())));
    let mut buf = Vec::new();
    let mut removed = 0;
    let mut skip = 0usize;
    loop {
        let ev = r.read_event_into(&mut buf)?;
        if skip > 0 {
            match ev {
                Event::Start(_) => skip += 1,
                Event::End(_) => skip -= 1,
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
            continue;
        }
        match &ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"Relationship" => {
                let external = attr(e, b"TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
                let hyperlink = attr(e, b"Type").is_some_and(|t| t.ends_with("/hyperlink"));
                if external && !hyperlink {
                    removed += 1;
                    if matches!(ev, Event::Start(_)) {
                        skip = 1;
                    }
                    buf.clear();
                    continue;
                }
            }
            _ => {}
        }
        w.write_event(ev)?;
        buf.clear();
    }
    Ok((w.into_inner().into_inner(), removed))
}

/// Externe Beziehungen aller `.rels`-Teile entfernen (für die Ersetzungsliste von `rewrite_zip`).
pub fn docx_external(docx: &[u8], replaced: &mut HashMap<String, Vec<u8>>) -> Result<usize> {
    let mut zip = zip::ZipArchive::new(Cursor::new(docx))?;
    let names: Vec<String> = zip.file_names().filter(|n| n.ends_with(".rels")).map(str::to_string).collect();
    let mut total = 0;
    for name in names {
        let xml = match replaced.get(&name) {
            Some(x) => x.clone(),
            None => read_limited(zip.by_name(&name)?)?,
        };
        let (new, n) = strip_rels(&xml)?;
        if n > 0 {
            total += n;
            replaced.insert(name, new);
        }
    }
    Ok(total)
}

/// Kopie der DOCX ohne externe Inhalte, für jeden Aufruf von LibreOffice.
pub fn docx_without_external(docx: &[u8]) -> Result<Vec<u8>> {
    let mut replaced = HashMap::new();
    if docx_external(docx, &mut replaced)? == 0 {
        return Ok(docx.to_vec());
    }
    crate::docx::rewrite_zip(docx, &replaced)
}

// ---------------------------------------------------------------------------
// ODT: xlink:href auf Dinge außerhalb der Datei
// ---------------------------------------------------------------------------

/// Elemente, deren `xlink:href` ein Hyperlink ist (wird nicht abgerufen).
const ODT_LINKS: [&[u8]; 2] = [b"text:a", b"draw:a"];

/// Elemente, die nur aus dem Verweis bestehen: ganz entfernen.
const ODT_DROP: [&[u8]; 3] = [b"text:section-source", b"office:dde-source", b"meta:template"];

/// Zeigt `href` auf einen Teil innerhalb der Datei (z. B. `Pictures/1.png`, `./Object 1`)?
fn odt_internal(href: &str, parts: &HashSet<String>) -> bool {
    let p = href.trim_start_matches("./");
    if p.is_empty() || p.contains(':') || p.starts_with('/') || p.starts_with('\\') || p.split(['/', '\\']).any(|s| s == "..") {
        return false;
    }
    let p = p.trim_end_matches('/');
    parts.contains(p) || parts.iter().any(|n| n.starts_with(&format!("{p}/")))
}

fn strip_odt_xml(xml: &[u8], parts: &HashSet<String>) -> Result<(Vec<u8>, usize)> {
    let mut r = reader(xml);
    let mut w = Writer::new(Cursor::new(Vec::with_capacity(xml.len())));
    let mut buf = Vec::new();
    let mut removed = 0;
    let mut skip = 0usize;
    loop {
        let ev = r.read_event_into(&mut buf)?;
        if skip > 0 {
            match ev {
                Event::Start(_) => skip += 1,
                Event::End(_) => skip -= 1,
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
            continue;
        }
        let out = match &ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let name = e.name();
                if ODT_DROP.contains(&name.as_ref()) {
                    removed += 1;
                    if matches!(ev, Event::Start(_)) {
                        skip = 1;
                    }
                    buf.clear();
                    continue;
                }
                match attr(e, b"xlink:href") {
                    Some(href) if !ODT_LINKS.contains(&name.as_ref()) && !odt_internal(&href, parts) => {
                        removed += 1;
                        let mut ne = BytesStart::new(String::from_utf8_lossy(name.as_ref()).into_owned());
                        for a in e.attributes().flatten() {
                            if a.key.as_ref() != b"xlink:href" {
                                ne.push_attribute(a);
                            }
                        }
                        if matches!(ev, Event::Empty(_)) { Event::Empty(ne) } else { Event::Start(ne) }
                    }
                    _ => ev.clone(),
                }
            }
            _ => ev.clone(),
        };
        w.write_event(out)?;
        buf.clear();
    }
    Ok((w.into_inner().into_inner(), removed))
}

/// Kopie der ODT ohne Verweise auf externe Bilder, Objekte, Schriften, Abschnitte und Vorlagen.
/// Liefert die Datei und die Zahl der entfernten Verweise.
pub fn odt_without_external(odt: &[u8]) -> Result<(Vec<u8>, usize)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(odt))?;
    let parts: HashSet<String> = zip.file_names().map(str::to_string).collect();
    let mut replaced = HashMap::new();
    let mut total = 0;
    let xml_names: Vec<String> = parts.iter().filter(|n| n.ends_with(".xml") && !n.starts_with("META-INF/")).cloned().collect();
    for name in xml_names {
        let xml = read_limited(zip.by_name(&name)?)?;
        let (new, n) = strip_odt_xml(&xml, &parts)?;
        if n > 0 {
            total += n;
            replaced.insert(name, new);
        }
    }
    if total == 0 {
        return Ok((odt.to_vec(), 0));
    }
    Ok((crate::docx::rewrite_zip(odt, &replaced)?, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (n, c) in parts {
            out.start_file(*n, o).unwrap();
            out.write_all(c).unwrap();
        }
        out.finish().unwrap().into_inner()
    }

    fn entry(zip_bytes: &[u8], name: &str) -> String {
        let mut z = zip::ZipArchive::new(Cursor::new(zip_bytes)).unwrap();
        let mut s = String::new();
        z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    const SMALL: Limits = Limits { entry: 1 << 20, total: 3 << 20, entries: 50 };

    #[test]
    fn zip_bomb_is_rejected() {
        // 2 MB Nullen packen auf wenige KB zusammen
        let bomb = zip_of(&[("word/document.xml", &vec![0u8; 2 << 20])]);
        assert!(bomb.len() < 20_000);
        let e = check_zip_with(&bomb, SMALL).unwrap_err().to_string();
        assert!(e.contains("word/document.xml"), "{e}");
    }

    #[test]
    fn zip_total_and_count_limits() {
        let mb = vec![b'a'; 900 << 10];
        let four: Vec<(String, &[u8])> = (0..4).map(|i| (format!("media/{i}.bin"), mb.as_slice())).collect();
        let parts: Vec<(&str, &[u8])> = four.iter().map(|(n, c)| (n.as_str(), *c)).collect();
        assert!(check_zip_with(&zip_of(&parts), SMALL).unwrap_err().to_string().contains("entpackt größer als 3 MB"));
        assert!(check_zip_with(&zip_of(&parts[..3]), SMALL).is_ok());
        let many: Vec<(String, &[u8])> = (0..51).map(|i| (format!("p{i}.xml"), &b"x"[..])).collect();
        let parts: Vec<(&str, &[u8])> = many.iter().map(|(n, c)| (n.as_str(), *c)).collect();
        assert!(check_zip_with(&zip_of(&parts), SMALL).unwrap_err().to_string().contains("50 Teilen"));
    }

    #[test]
    fn normal_docx_passes() {
        check_zip(&crate::docx::tests::fixture()).unwrap();
    }

    #[test]
    fn external_rels_removed_hyperlinks_kept() {
        let rels = br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="http://10.0.0.1/x.png" TargetMode="External"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://www.ris.bka.gv.at/" TargetMode="External"/></Relationships>"#;
        let settings = br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/attachedTemplate" Target="file:///etc/passwd" TargetMode="External"/></Relationships>"#;
        let docx = zip_of(&[("word/_rels/document.xml.rels", rels), ("word/_rels/settings.xml.rels", settings), ("word/document.xml", b"<w:document/>")]);
        let clean = docx_without_external(&docx).unwrap();
        let r = entry(&clean, "word/_rels/document.xml.rels");
        assert!(!r.contains("10.0.0.1") && r.contains("ris.bka.gv.at") && r.contains("styles.xml"), "{r}");
        assert!(!entry(&clean, "word/_rels/settings.xml.rels").contains("passwd"));
        assert_eq!(entry(&clean, "word/document.xml"), "<w:document/>");
        // ohne externe Inhalte bleibt die Datei Byte für Byte gleich
        let fx = crate::docx::tests::fixture();
        assert_eq!(docx_without_external(&fx).unwrap(), fx);
    }

    #[test]
    fn odt_external_references_removed() {
        let content = br#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:xlink="http://www.w3.org/1999/xlink"><office:body><office:text>
<text:p><text:a xlink:type="simple" xlink:href="https://www.bab.gv.at/">Link</text:a></text:p>
<draw:frame><draw:image xlink:href="Pictures/1.png" xlink:type="simple"/></draw:frame>
<draw:frame><draw:image xlink:href="http://10.0.0.1/x.png" xlink:type="simple"/></draw:frame>
<draw:frame><draw:image xlink:href="../../users.txt"/></draw:frame>
<text:section text:name="S"><text:section-source xlink:href="file:///etc/passwd"/><text:p>alt</text:p></text:section>
</office:text></office:body></office:document-content>"#;
        let odt = zip_of(&[("mimetype", b"application/vnd.oasis.opendocument.text"), ("content.xml", content), ("Pictures/1.png", b"png")]);
        let (clean, n) = odt_without_external(&odt).unwrap();
        assert_eq!(n, 3);
        let c = entry(&clean, "content.xml");
        assert!(c.contains("https://www.bab.gv.at/") && c.contains("Pictures/1.png"), "{c}");
        assert!(!c.contains("10.0.0.1") && !c.contains("users.txt") && !c.contains("passwd") && !c.contains("section-source"), "{c}");
        assert!(c.contains("<text:p>alt</text:p>"));
        // mimetype bleibt der erste Eintrag
        assert_eq!(zip::ZipArchive::new(Cursor::new(&clean)).unwrap().file_names().next(), Some("mimetype"));
    }
}
