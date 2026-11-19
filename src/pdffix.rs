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

/// Korrigiert das PDF an Ort und Stelle. Wird nichts gefunden, bleibt die Datei unberührt.
pub fn fix_structure(path: &Path) -> Result<usize> {
    let mut doc = Document::load(path).context("PDF konnte nicht gelesen werden")?;
    let n = fix_document(&mut doc)?;
    if n > 0 {
        let tmp = path.with_extension("pdf.tmp");
        doc.save(&tmp).context("Korrigiertes PDF konnte nicht geschrieben werden")?;
        std::fs::rename(&tmp, path)?;
    }
    Ok(n)
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
        let moved = fix_structure(&pdf).unwrap();
        let (_, after) = pdfcheck::basic_checks(&pdf).unwrap();
        println!("LibreOffice-Ausgabe: {:?}", check(&before, "Falsch verschachtelte Struktur").map(|c| &c.detail));
        println!("verschoben: {moved}");
        assert!(check(&after, "Verschachtelung").is_some_and(|c| c.ok), "{:?}", after.iter().filter(|c| !c.ok).map(|c| &c.title).collect::<Vec<_>>());
        assert!(check(&after, "Standard-Tags").is_some_and(|c| c.ok));
        // zweiter Lauf darf nichts mehr finden
        assert_eq!(fix_structure(&pdf).unwrap(), 0);
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
        println!("verschoben: {}", fix_structure(&out).unwrap());
        let (_, checks) = pdfcheck::basic_checks(&out).unwrap();
        for c in checks.iter().filter(|c| !c.ok) {
            println!("offen: {} – {}", c.title, c.detail);
        }
    }
}
