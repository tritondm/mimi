// SPDX-License-Identifier: EUPL-1.2
// Copyright (c) 2026 example.org

//! Nachbearbeitung des PDFs von LibreOffice: Strukturbaum geradeziehen.
//!
//! LibreOffice hängt Word-Rahmen (`w:framePr`) und Textfelder als `Div` in den
//! Absatz, an dem sie verankert sind. Ein Absatz (`P`, `H1` …) darf aber keine
//! Bereiche, Absätze oder Überschriften enthalten (ISO 32000-2 Anhang L,
//! PDF/UA-2, PAC). Hier werden solche Elemente eine Ebene nach oben verschoben,
//! direkt hinter den Absatz. Am Seiteninhalt ändert sich nichts – das Layout
//! bleibt exakt so, wie LibreOffice es gesetzt hat.
//!
//! Die Korrektur sucht nach der Regel, nicht nach einem bestimmten Muster von
//! LibreOffice: erzeugt eine spätere Version den Baum von sich aus richtig,
//! findet sie nichts und lässt das PDF unverändert.

use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::path::Path;

/// Elemente, die keine Blockelemente enthalten dürfen.
pub const PARA_TYPES: &[&[u8]] = &[b"P", b"H", b"H1", b"H2", b"H3", b"H4", b"H5", b"H6"];

/// Blockelemente, die nicht in einem Absatz stehen dürfen.
pub const BLOCK_TYPES: &[&[u8]] = &[
    b"Div", b"Sect", b"Art", b"Part", b"BlockQuote", b"P", b"H", b"H1", b"H2", b"H3", b"H4", b"H5", b"H6", b"L",
    b"Table", b"TOC", b"Index",
];

/// Löst einen Strukturtyp über die RoleMap auf (mehrstufig, mit Schutz vor Schleifen).
pub fn resolve_role(role_map: Option<&Dictionary>, s: &[u8]) -> Vec<u8> {
    let mut cur = s.to_vec();
    if let Some(rm) = role_map {
        for _ in 0..10 {
            match rm.get(&cur) {
                Ok(Object::Name(n)) if *n != cur => cur = n.clone(),
                _ => break,
            }
        }
    }
    cur
}

fn kids_of(d: &Dictionary) -> Vec<Object> {
    match d.get(b"K") {
        Ok(Object::Array(a)) => a.clone(),
        Ok(o) => vec![o.clone()],
        Err(_) => Vec::new(),
    }
}

/// Typ eines Strukturelements (nach RoleMap) – `None`, wenn das Objekt keins ist
/// (z. B. Verweis auf markierten Inhalt `MCR` oder ein Objekt `OBJR`).
fn elem_role(doc: &Document, role_map: Option<&Dictionary>, id: ObjectId) -> Option<Vec<u8>> {
    let d = doc.get_object(id).ok()?.as_dict().ok()?;
    match d.get(b"S") {
        Ok(Object::Name(s)) => Some(resolve_role(role_map, s)),
        _ => None,
    }
}

/// Sucht den ersten Absatz mit Blockelementen darin: (Eltern, Absatz, zu verschiebende Kinder).
fn find_nested(doc: &Document, role_map: Option<&Dictionary>, root: ObjectId) -> Option<(ObjectId, ObjectId, Vec<ObjectId>)> {
    let mut stack: Vec<(ObjectId, ObjectId)> = Vec::new(); // (Eltern, Element)
    let mut seen = std::collections::HashSet::new();
    let root_dict = doc.get_object(root).ok()?.as_dict().ok()?;
    for k in kids_of(root_dict).iter().rev() {
        if let Object::Reference(id) = k {
            stack.push((root, *id));
        }
    }
    while let Some((parent, id)) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(role) = elem_role(doc, role_map, id) else { continue };
        let Ok(d) = doc.get_object(id).and_then(|o| o.as_dict()) else { continue };
        let kids: Vec<ObjectId> = kids_of(d).iter().filter_map(|k| if let Object::Reference(r) = k { Some(*r) } else { None }).collect();
        if PARA_TYPES.contains(&role.as_slice()) && parent != root {
            let moving: Vec<ObjectId> = kids
                .iter()
                .copied()
                .filter(|k| elem_role(doc, role_map, *k).is_some_and(|r| BLOCK_TYPES.contains(&r.as_slice())))
                .collect();
            if !moving.is_empty() {
                return Some((parent, id, moving));
            }
        }
        for k in kids.iter().rev() {
            stack.push((id, *k));
        }
    }
    None
}

/// Zieht den Strukturbaum im Speicher gerade. Gibt die Zahl der verschobenen Elemente zurück.
pub fn fix_document(doc: &mut Document) -> Result<usize> {
    let root = match doc.catalog()?.get(b"StructTreeRoot") {
        Ok(Object::Reference(id)) => *id,
        _ => return Ok(0), // nicht getaggt oder ungewöhnlich gespeichert: nichts anfassen
    };
    let role_map = doc
        .get_object(root)?
        .as_dict()?
        .get(b"RoleMap")
        .ok()
        .and_then(|o| match o {
            Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| o.as_dict().ok()).cloned(),
            Object::Dictionary(d) => Some(d.clone()),
            _ => None,
        });
    let mut moved = 0;
    // jede Runde löst genau einen Absatz; Obergrenze gegen kaputte Bäume
    for _ in 0..10_000 {
        let Some((parent, para, moving)) = find_nested(doc, role_map.as_ref(), root) else {
            return Ok(moved);
        };
        // 1. aus dem Absatz entfernen
        let para_dict = doc.get_object_mut(para)?.as_dict_mut()?;
        let remaining: Vec<Object> = kids_of(para_dict).into_iter().filter(|k| !matches!(k, Object::Reference(r) if moving.contains(r))).collect();
        let para_empty = remaining.is_empty();
        para_dict.set("K", Object::Array(remaining));
        // 2. beim Eltern direkt hinter dem Absatz einfügen (leerer Absatz fällt weg)
        let parent_dict = doc.get_object_mut(parent)?.as_dict_mut()?;
        let mut new_kids = Vec::new();
        for k in kids_of(parent_dict) {
            if matches!(k, Object::Reference(r) if r == para) {
                if !para_empty {
                    new_kids.push(k);
                }
                new_kids.extend(moving.iter().map(|m| Object::Reference(*m)));
            } else {
                new_kids.push(k);
            }
        }
        parent_dict.set("K", Object::Array(new_kids));
        // 3. Eltern-Verweis der verschobenen Elemente
        for m in &moving {
            doc.get_object_mut(*m)?.as_dict_mut()?.set("P", Object::Reference(parent));
        }
        moved += moving.len();
    }
    anyhow::bail!("Strukturbaum ließ sich nicht korrigieren (zu viele Verschachtelungen)")
}

/// Sucht im Inhaltsstrom `/Tag <</MCID n>> BDC` und liefert (Start, Ende, MCID).
fn find_mcid_bdc(content: &[u8]) -> Vec<(usize, usize, i64)> {
    let mut out = Vec::new();
    let ws = |c: u8| c.is_ascii_whitespace();
    let mut i = 0;
    while let Some(off) = content[i..].windows(5).position(|w| w == b"/MCID") {
        let at = i + off;
        i = at + 5;
        // rückwärts: nur Leerraum bis „<<“, davor Leerraum und ein Name
        let mut b = at;
        while b > 0 && ws(content[b - 1]) {
            b -= 1;
        }
        if b < 2 || &content[b - 2..b] != b"<<" {
            continue;
        }
        b -= 2;
        while b > 0 && ws(content[b - 1]) {
            b -= 1;
        }
        let name_end = b;
        while b > 0 && !ws(content[b - 1]) && content[b - 1] != b'/' && !b"<>[]()".contains(&content[b - 1]) {
            b -= 1;
        }
        if b == 0 || content[b - 1] != b'/' || b == name_end {
            continue;
        }
        let start = b - 1;
        // vorwärts: Zahl, „>>“, „BDC“
        let mut j = i;
        while j < content.len() && ws(content[j]) {
            j += 1;
        }
        let ns = j;
        while j < content.len() && content[j].is_ascii_digit() {
            j += 1;
        }
        let Ok(mcid) = std::str::from_utf8(&content[ns..j]).unwrap_or("").parse::<i64>() else { continue };
        while j < content.len() && ws(content[j]) {
            j += 1;
        }
        if !content[j..].starts_with(b">>") {
            continue;
        }
        j += 2;
        while j < content.len() && ws(content[j]) {
            j += 1;
        }
        if !content[j..].starts_with(b"BDC") {
            continue;
        }
        out.push((start, j + 3, mcid));
        i = j + 3;
    }
    out
}

/// Macht Abbildungen mit dem Alternativtext [`crate::docx::DECORATIVE_ALT`] zu
/// Artefakten: LibreOffice übernimmt „dekorativ“ nur bei Bildern, nicht bei
/// gezeichneten Formen (Zierbalken, Rahmen). Die Export-Kopie kennzeichnet diese
/// deshalb über den Alternativtext; hier wird ihr markierter Inhalt zu
/// `/Artifact BMC` und das Strukturelement entfällt. Das Seitenbild bleibt gleich.
pub fn artifact_decorative(doc: &mut Document) -> Result<usize> {
    let root = match doc.catalog()?.get(b"StructTreeRoot") {
        Ok(Object::Reference(id)) => *id,
        _ => return Ok(0),
    };
    let marked: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter_map(|(id, o)| {
            let d = o.as_dict().ok()?;
            let alt = d.get(b"Alt").ok()?;
            (d.get(b"S").is_ok() && crate::pdfcheck::text_string(alt) == crate::docx::DECORATIVE_ALT).then_some(*id)
        })
        .collect();
    if marked.is_empty() {
        return Ok(0);
    }
    // markierte Inhalte je Element: (Seite, MCID)
    let mut elems: Vec<(ObjectId, Vec<(ObjectId, i64)>)> = Vec::new();
    for id in marked {
        let d = doc.get_object(id)?.as_dict()?.clone();
        let pg = d.get(b"Pg").ok().and_then(|o| o.as_reference().ok());
        let mut mcids = Vec::new();
        let mut ok = true;
        for k in kids_of(&d) {
            let mcr = match &k {
                Object::Integer(m) => {
                    match pg {
                        Some(p) => mcids.push((p, *m)),
                        None => ok = false,
                    }
                    continue;
                }
                Object::Reference(r) => doc.get_object(*r).ok().and_then(|o| o.as_dict().ok()).cloned(),
                Object::Dictionary(kd) => Some(kd.clone()),
                _ => None,
            };
            match mcr {
                Some(kd) if kd.get(b"Type").ok().and_then(|t| t.as_name().ok()) == Some(b"MCR".as_slice()) => {
                    let p = kd.get(b"Pg").ok().and_then(|o| o.as_reference().ok()).or(pg);
                    match (p, kd.get(b"MCID").ok().and_then(|m| m.as_i64().ok())) {
                        (Some(p), Some(m)) => mcids.push((p, m)),
                        _ => ok = false,
                    }
                }
                // Unterelemente oder Objektverweise: nicht anfassen
                _ => ok = false,
            }
        }
        if ok && !mcids.is_empty() {
            elems.push((id, mcids));
        }
    }
    // nur Elemente, deren Inhalt im Seiteninhalt auffindbar ist
    let mut found: std::collections::HashMap<ObjectId, std::collections::HashSet<i64>> = std::collections::HashMap::new();
    for (_, mc) in &elems {
        for (p, _) in mc {
            if !found.contains_key(p) {
                let c = doc.get_page_content(*p);
                found.insert(*p, find_mcid_bdc(&c).into_iter().map(|(_, _, m)| m).collect());
            }
        }
    }
    elems.retain(|(_, mc)| mc.iter().all(|(p, m)| found.get(p).is_some_and(|s| s.contains(m))));
    if elems.is_empty() {
        return Ok(0);
    }
    // 1. Seiteninhalt: BDC → /Artifact BMC
    let mut per_page: std::collections::HashMap<ObjectId, std::collections::HashSet<i64>> = std::collections::HashMap::new();
    for (_, mc) in &elems {
        for (p, m) in mc {
            per_page.entry(*p).or_default().insert(*m);
        }
    }
    for (page, ms) in &per_page {
        let c = doc.get_page_content(*page);
        let mut out = Vec::with_capacity(c.len());
        let mut last = 0;
        for (s, e, m) in find_mcid_bdc(&c) {
            if ms.contains(&m) {
                out.extend_from_slice(&c[last..s]);
                out.extend_from_slice(b"/Artifact BMC");
                last = e;
            }
        }
        out.extend_from_slice(&c[last..]);
        doc.change_page_content(*page, out)?;
    }
    // 2. ParentTree: Einträge leeren (nur flache /Nums-Liste, wie LibreOffice sie schreibt)
    let pt = doc.get_object(root)?.as_dict()?.get(b"ParentTree").ok().and_then(|o| o.as_reference().ok());
    if let Some(pt) = pt {
        let nums = doc.get_object(pt).ok().and_then(|o| o.as_dict().ok()).and_then(|d| d.get(b"Nums").ok()).and_then(|n| n.as_array().ok()).cloned();
        if let Some(nums) = nums {
            for (page, ms) in &per_page {
                let Some(key) = doc.get_object(*page).ok().and_then(|o| o.as_dict().ok()).and_then(|d| d.get(b"StructParents").ok()).and_then(|k| k.as_i64().ok()) else { continue };
                let Some(pos) = nums.chunks(2).position(|c| c[0].as_i64().ok() == Some(key)) else { continue };
                if let Some(Object::Reference(arr)) = nums.get(pos * 2 + 1) {
                    if let Ok(Object::Array(a)) = doc.get_object_mut(*arr) {
                        for m in ms {
                            if let Some(slot) = a.get_mut(*m as usize) {
                                *slot = Object::Null;
                            }
                        }
                    }
                }
            }
        }
    }
    // 3. aus dem Strukturbaum entfernen
    let n = elems.len();
    for (id, _) in elems {
        let parent = doc.get_object(id)?.as_dict()?.get(b"P").ok().and_then(|o| o.as_reference().ok());
        if let Some(parent) = parent {
            if let Ok(pd) = doc.get_object_mut(parent).and_then(|o| o.as_dict_mut()) {
                let kids: Vec<Object> = kids_of(pd).into_iter().filter(|k| !matches!(k, Object::Reference(r) if *r == id)).collect();
                pd.set("K", Object::Array(kids));
            }
        }
        doc.objects.remove(&id);
    }
    Ok(n)
}

/// Ergebnis der Nachbearbeitung.
#[derive(Default, Debug)]
pub struct FixResult {
    /// aus Absätzen gelöste Bereiche
    pub moved: usize,
    /// dekorative Formen, die zu Artefakten wurden
    pub artifacts: usize,
}

/// Korrigiert das PDF an Ort und Stelle. Wird nichts gefunden, bleibt die Datei unberührt.
pub fn fix_structure(path: &Path) -> Result<FixResult> {
    let mut doc = Document::load(path).context("PDF konnte nicht gelesen werden")?;
    let artifacts = artifact_decorative(&mut doc)?;
    let moved = fix_document(&mut doc)?;
    if moved > 0 || artifacts > 0 {
        let tmp = path.with_extension("pdf.tmp");
        doc.save(&tmp).context("Korrigiertes PDF konnte nicht geschrieben werden")?;
        std::fs::rename(&tmp, path)?;
    }
    Ok(FixResult { moved, artifacts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdfcheck;

    fn soffice_pdf(docx: &[u8], dir: &Path) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        // LibreOffice braucht eine absolute file://-URL für das Profil
        let dir = &std::fs::canonicalize(dir).unwrap();
        let input = dir.join("test.docx");
        std::fs::write(&input, docx).unwrap();
        let profile = format!("-env:UserInstallation=file://{}", dir.join("lo-profile").display());
        let filter = r#"pdf:writer_pdf_Export:{"UseTaggedPDF":{"type":"boolean","value":"true"},"PDFUACompliance":{"type":"boolean","value":"true"}}"#;
        let st = std::process::Command::new(std::env::var("SOFFICE").unwrap_or("soffice".into()))
            .args([profile.as_str(), "--headless", "--norestore", "--convert-to", filter, "--outdir"])
            .arg(dir)
            .arg(&input)
            .output()
            .expect("LibreOffice nicht gefunden");
        assert!(st.status.success(), "{}", String::from_utf8_lossy(&st.stderr));
        dir.join("test.pdf")
    }

    #[test]
    fn finds_marked_content() {
        let c = b"q /Figure<</MCID 3>>BDC 0 0 m EMC /P <</MCID 12>> BDC (x) Tj EMC /Span<</ActualText(a)/MCID 4>>BDC EMC";
        let f = find_mcid_bdc(c);
        assert_eq!(f.iter().map(|x| x.2).collect::<Vec<_>>(), vec![3, 12]);
        assert_eq!(&c[f[0].0..f[0].1], b"/Figure<</MCID 3>>BDC");
    }

    fn check<'a>(checks: &'a [pdfcheck::Check], title: &str) -> Option<&'a pdfcheck::Check> {
        checks.iter().find(|c| c.title == title)
    }

    /// Regressionstest gegen die installierte LibreOffice-Version – nach jedem
    /// LibreOffice-Update laufen lassen: `cargo test --release -- --ignored`.
    /// Prüft, dass Vorbereitung + Nachbearbeitung zusammen ein sauber
    /// verschachteltes PDF ergeben, egal ob LibreOffice den Fehler noch macht.
    #[test]
    #[ignore]
    fn libreoffice_roundtrip() {
        let dir = std::path::PathBuf::from("target/lo-roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let (prep, _) = crate::docx::prepare_for_pdf(&crate::docx::tests::fixture()).unwrap();
        let pdf = soffice_pdf(&prep, &dir);
        let (_, before) = pdfcheck::basic_checks(&pdf).unwrap();
        let moved = fix_structure(&pdf).unwrap().moved;
        let (_, after) = pdfcheck::basic_checks(&pdf).unwrap();
        println!("LibreOffice-Ausgabe: {:?}", check(&before, "Falsch verschachtelte Struktur").map(|c| &c.detail));
        println!("verschoben: {moved}");
        assert!(check(&after, "Verschachtelung").is_some_and(|c| c.ok), "{:?}", after.iter().filter(|c| !c.ok).map(|c| &c.title).collect::<Vec<_>>());
        assert!(check(&after, "Standard-Tags").is_some_and(|c| c.ok));
        // zweiter Lauf darf nichts mehr finden
        assert_eq!(fix_structure(&pdf).unwrap().moved, 0);
    }

    /// Beliebiges PDF korrigieren: `RAMPE_TEST_PDF=a.pdf cargo test --release fix_file -- --ignored --nocapture`
    /// (schreibt a.fixed.pdf daneben).
    #[test]
    #[ignore]
    fn fix_file() {
        let Ok(p) = std::env::var("RAMPE_TEST_PDF") else { return };
        let src = std::path::PathBuf::from(p);
        let out = src.with_extension("fixed.pdf");
        std::fs::copy(&src, &out).unwrap();
        println!("{:?}", fix_structure(&out).unwrap());
        let (_, checks) = pdfcheck::basic_checks(&out).unwrap();
        for c in checks.iter().filter(|c| !c.ok) {
            println!("offen: {} – {}", c.title, c.detail);
        }
    }
}
