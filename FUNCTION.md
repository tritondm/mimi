# Wie Rampe funktioniert

Rampe macht aus einem Word-Bericht (.docx) ein barrierefreies PDF nach **PDF/UA-1**.
Die Mitarbeitenden laden den Bericht im Browser hoch, ergänzen dort, was nur ein Mensch
wissen kann (z. B. Alternativtexte), und bekommen ein geprüftes PDF zurück.

Installation und Betrieb: [INSTALL.md](INSTALL.md) · Bedienung und Einstellungen: [README.md](README.md)

## Ablauf

![Ablauf: Hochladen, Analysieren, Korrigieren, Vorbereiten, PDF erzeugen, Prüfen, Herunterladen](docs/img/ablauf.svg)

| Schritt | Wo | Was passiert |
|---|---|---|
| 1 Hochladen | Browser → `POST /api/upload` | Die Datei wird als `original.docx` und `current.docx` abgelegt. |
| 2 Analysieren | `docx::analyze` | Bilder, Tabellen, Links, Überschriften, Farben, Titel und Sprache werden gelesen und als Liste von Befunden an den Browser geschickt. |
| 3 Korrigieren | Browser → `POST /api/job/{id}/apply` | Die Änderungen werden mit `docx::apply` in `current.docx` geschrieben. Danach wird die Datei sofort neu analysiert. Ist sie nicht mehr lesbar, wird nicht gespeichert. |
| 4 Vorbereiten | `docx::prepare_for_pdf` | Für den Export entsteht eine **Kopie** mit Anpassungen, die nur LibreOffice betreffen (siehe unten). |
| 5 PDF erzeugen | LibreOffice ohne Oberfläche | Aus der Kopie entstehen `out/current.pdf` (PDF/UA, getaggt) und `out/current.odt`. |
| 6 Prüfen | `pdfcheck::basic_checks` + veraPDF | Basisprüfung und, falls installiert, die vollständige PDF/UA-1-Prüfung. Das Ergebnis liegt in `out/report.json`. |

Die herunterladbare DOCX ist immer `current.docx`, also die Fassung mit den Korrekturen aus
Schritt 3 und ohne die Anpassungen aus Schritt 4. Sie bleibt eine normale Word-Datei.

## Bausteine

| Datei | Aufgabe |
|---|---|
| `src/main.rs` | Webserver (axum): Routen, Anmeldung, Ablage, Aufruf von LibreOffice |
| `src/auth.rs` | Benutzer, Passwörter (Argon2id), Sitzungen, Kommandozeilenbefehle |
| `src/docx.rs` | Lesen, Korrigieren und Vorbereiten der Word-Datei |
| `src/pdfcheck.rs` | Basisprüfung des PDFs, Aufruf von veraPDF |
| `static/index.html` | Die gesamte Oberfläche (eine Seite, ohne externe Abhängigkeiten) |
| `static/admin.html` | Benutzerverwaltung |
| `deploy/` | Deploy-Skript, systemd-Dienst, nginx-Konfiguration |

### Wie die Word-Datei bearbeitet wird

Eine DOCX ist ein ZIP-Archiv mit XML-Dateien (`word/document.xml`, `word/styles.xml`,
Kopf- und Fußzeilen …). Rampe liest das XML **ereignisbasiert** (quick-xml) und reicht alles
unverändert durch. Neu geschrieben werden nur die Elemente, die sich tatsächlich ändern.
Alle anderen ZIP-Einträge werden roh kopiert, ohne sie neu zu packen. So bleibt alles,
was Rampe nicht versteht (Diagramme, Makros, Erweiterungen von Word), Byte für Byte erhalten.

## Was geprüft und korrigiert wird

| Bereich | Erkennung | Korrektur in Rampe |
|---|---|---|
| Bilder, Formen, Diagramme | fehlender oder nichtssagender Alternativtext („Bild 3“, Dateiname) | Text eingeben, Bildunterschrift übernehmen oder „dekorativ“ |
| Tabellen | erste Zeile nicht als Kopfzeile markiert | Häkchen → `w:tblHeader` |
| Dokument | Titel und Hauptsprache fehlen | Felder → `docProps/core.xml`, Standardsprache in `styles.xml` |
| Links | Link ohne QuickInfo (v. a. in Verzeichnissen) | QuickInfo aus dem Linktext. LibreOffice macht daraus die Linkbeschreibung `/Contents` |
| Überschriften | übersprungene Ebene (H4 nach H2) | Vorlage der richtigen Ebene setzen |
| Schriftfarben | Kontrast unter 4,5 : 1 gegen Weiß | Farbton abdunkeln, bis 4,6 : 1 erreicht ist |
| Struktur | keine Überschriften, fett statt Überschrift, getippte Aufzählungen, Leerabsätze | nur Hinweis, in Word beheben |

## Vorbereitung für LibreOffice

LibreOffice erzeugt das PDF. Ein paar Eigenheiten beim Umwandeln von Word-Dateien würden
aber zu PDF/UA-Fehlern führen, auch wenn das Word-Dokument selbst in Ordnung ist. Rampe
behebt sie in einer Kopie, die nur für den Export entsteht:

![Vergleich der PDF-Tags ohne und mit Vorbereitung](docs/img/vorbereitung.svg)

| Anpassung | Warum | Funktion |
|---|---|---|
| Eigene Vorlagen mit dem Namen eines PDF-Standard-Tags (z. B. „TD“, „Quote“) bekommen den Zusatz „Vorlage“ | LibreOffice macht aus jedem Vorlagennamen einen Tag und leitet ihn per RoleMap um. Eine Vorlage „TD“ würde so **alle** Tabellenzellen zu Absätzen machen (veraPDF 7.1-7, 7.2-10, 7.2-43). | `rename_std_styles` |
| Die Vorlage „Beschriftung“ (`caption`) heißt in der Kopie „Bildunterschrift (Rampe)“ | LibreOffice taggt Beschriftungen als `Caption`, aber frei stehend statt innerhalb von `Figure`/`Table`. PAC bemängelt das. Als normaler Absatz ist es korrekt. Nummerierung (SEQ-Felder) und Verzeichnisse bleiben gleich. | `rename_caption_style` |
| Links und Verweis-Felder (`STYLEREF`, `REF`, `PAGEREF` …) in Kopf- und Fußzeilen werden zu Text | Kopf- und Fußzeilen sind im PDF Artefakte. Ein Link darin ist ungetaggt, und zwar auf jeder Seite (veraPDF 7.18.5-1). | `unlink_header_footer` |
| Literaturverzeichnis-Felder (Word `BIBLIOGRAPHY`, Citavi, Zotero, EndNote) werden zu Text | LibreOffice baut sonst ein geschütztes Verzeichnis, das anders aussieht als in Word und als Inhaltsverzeichnis (`TOC`) getaggt wird. | `unlink_bibliography` |

Welche Anpassungen ein Bericht bekommt, zeigt Rampe vor dem Export im Reiter
„PDF erzeugen“ an (`pdf_prep` in der Analyse).

## Prüfung des PDFs

1. **Basisprüfung** (`pdfcheck.rs`, immer): getaggt, Sprache, Titel und dessen Anzeige,
   PDF/UA-Kennung, umgeleitete Standard-Tags, Alternativtexte, Überschriften und
   Ebenensprünge, Tabellenköpfe, eingebettete Schriften, Linkbeschreibungen, ungetaggte Links.
2. **veraPDF** (falls installiert): vollständige maschinelle Prüfung nach PDF/UA-1.

Was keine Software prüfen kann, z. B. ob ein Alternativtext sinnvoll ist oder ob die
Lesereihenfolge stimmt, bleibt beim Menschen. Den abschließenden Test mit **PAC** (nur Windows)
macht die Mitarbeiterin bzw. der Mitarbeiter.

## Praxistest: Praxistest-Abschlussbericht (November 2026)

100 Seiten, 64 Bilder, 12 Tabellen, 113 Links in den Verzeichnissen. Prüfung mit veraPDF 1.30.4 (PDF/UA-1):

| | Word-Datei direkt | nur Vorbereitung | Vorbereitung + Korrekturen in Rampe |
|---|---|---|---|
| Vorlage „TD“ umgeleitet (7.1-7, 7.2-10, 7.2-43) | ✗ | ✓ | ✓ |
| Links in Fußzeilen (7.18.5-1) | ✗ | ✓ | ✓ |
| 37 Bilder ohne Alternativtext (7.3-1) | ✗ | ✗ | ✓ |
| 169 Links ohne Beschreibung (7.18.1-2, 7.18.5-2) | ✗ | ✗ | ✓ |
| 4 übersprungene Überschriftenebenen (7.4.2-1) | ✗ | ✗ | ✓ |
| **Ergebnis** | 8 Regeln verletzt | 4 Regeln verletzt | **konform** |

Alle 111 Verzeichniseinträge (Inhalt, Tabellen, Abbildungen) sind im PDF verlinkt.
Die 63 Beschriftungen sind normale Absätze statt frei stehender `Caption`.

## Testen

```bash
cargo test --release                     # Unit-Tests der XML-Umbauten
```

Einen echten Bericht ohne Oberfläche durchspielen: Mit `RAMPE_TEST_APPLY=1` wird vorher
alles angewendet, was man in Rampe anklicken kann. Alternativtexte werden dabei Platzhalter.

```bash
RAMPE_TEST_APPLY=1 RAMPE_TEST_IN=bericht.docx RAMPE_TEST_OUT=/tmp/rt \
  cargo test --release write_fixture -- --ignored --nocapture

cd /tmp/rt
soffice --headless --convert-to 'pdf:writer_pdf_Export:{"UseTaggedPDF":{"type":"boolean","value":"true"},"PDFUACompliance":{"type":"boolean","value":"true"}}' vorbereitet.docx
verapdf -f ua1 --format text vorbereitet.pdf
```

Ohne `RAMPE_TEST_IN` wird ein kleines eingebautes Beispieldokument verwendet.

## Offene Punkte

- **Word-Rahmen** (`w:framePr`, z. B. Titelblock auf Seite 1, Impressum): LibreOffice setzt
  ihren Inhalt als `Div` in einen Absatz. veraPDF stört das nicht, PAC schon. Geplant ist,
  sie in normale Absätze umzuwandeln und die Lage über den Abstand nachzubilden.
- **Zeilenhöhen in Tabellen**: Weichen sie vom Original ab, liegt das meist an fehlenden
  Schriften (siehe [INSTALL.md](INSTALL.md#schriften)). Feste Zeilenhöhen schneiden
  mehrzeilige Zellen ab und sind deshalb keine Lösung.
- Weitere Grenzen des Prototyps: siehe [README.md](README.md#bekannte-grenzen-des-prototyps).
