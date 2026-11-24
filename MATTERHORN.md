# Leo und das Matterhorn-Protokoll

**Deutsch** · [English](MATTERHORN.en.md)

*Leo – Leichte Erstellung Ohne Barrieren*

Das **Matterhorn-Protokoll** (PDF Association, Version 1.1) beschreibt in 31 Prüfpunkten,
woran ein PDF scheitert, das nicht PDF/UA-1 (ISO 14289-1) entspricht. Ein Teil der
Fehlerbedingungen ist maschinell prüfbar (veraPDF, PAC), der Rest nur von Menschen.

Diese Übersicht zeigt für jeden Prüfpunkt, was Leo tut und was beim Menschen bleibt.
Stand: November 2026, Leo mit LibreOffice 26.8 und veraPDF 1.30.

## Legende

| Wertung | Bedeutung |
|---|---|
| **ja** | Leo erkennt und behebt das (in der Word-Datei, der Export-Kopie oder im fertigen PDF) |
| **teilweise** | Leo behebt einen Teil, gibt Hinweise oder prüft nur; der Rest ist Handarbeit |
| **nein** | nicht abgedeckt, nur Handarbeit |
| **n. z.** | nicht zutreffend für Berichte aus Word (die Funktion kommt im erzeugten PDF nicht vor) |

**Wo Leo eingreift:**

| Stufe | Was |
|---|---|
| **Analyse** | Befunde zur Word-Datei, Korrektur per Klick oder Hinweis zum Beheben in Word |
| **Vorbereitung** | automatische Anpassungen einer Export-Kopie für LibreOffice (die Word-Datei bleibt unverändert) |
| **Nachbearbeitung** | Korrekturen am Strukturbaum des fertigen PDFs |
| **Basisprüfung** | Leos eigene Prüfung des PDFs |
| **veraPDF** | vollständige maschinelle Prüfung nach PDF/UA-1 |

## Übersicht

| Nr. | Prüfpunkt | Leo | Was Leo tut | Bleibt beim Menschen |
|---|---|---|---|---|
| 01 | Echter Inhalt getaggt, Artefakte als Artefakt | **teilweise** | LibreOffice taggt den Inhalt, Kopf- und Fußzeilen werden Artefakte. Vorbereitung: Links und Felder in Kopf-/Fußzeilen werden Text, Bilder und Formen dort dekorativ. Nachbearbeitung: als dekorativ markierte Formen werden Artefakte. veraPDF prüft auf ungetaggten Inhalt. | Ist etwas fälschlich Artefakt, oder ist Schmuck als Inhalt getaggt? |
| 02 | Rollenzuordnung (RoleMap) | **ja** | Vorbereitung: eigene Formatvorlagen mit dem Namen eines Standard-Tags („TD“, „Quote“ …) werden umbenannt. Basisprüfung „Standard-Tags umgeleitet“, veraPDF. | – |
| 03 | Flackern | **n. z.** | Berichte aus Word enthalten keine Animationen. | – |
| 04 | Farbe und Kontrast | **teilweise** | Information nur über Farbe wird **nicht** erkannt. Zusätzlich (WCAG 1.4.3, nicht Matterhorn): Kontrast von Schriftfarben gegen Weiß, Korrektur durch Abdunkeln. | Wird Information nur durch Farbe, Form oder Lage vermittelt? Kontrast auf farbigem Hintergrund und in Grafiken. |
| 05 | Ton | **n. z.** | – | – |
| 06 | Metadaten | **ja** | Analyse: Dokumenttitel wird abgefragt und gesetzt. LibreOffice schreibt XMP-Metadaten und PDF/UA-Kennung. Basisprüfung: „XMP-Metadaten fehlen“, „PDF/UA-Kennung fehlt“, „Dokumenttitel fehlt“. | Beschreibt der Titel das Dokument wirklich? |
| 07 | Dictionary (Anzeige des Titels) | **ja** | LibreOffice setzt `DisplayDocTitle`. Basisprüfung „Fenster zeigt Dateinamen statt Titel“. | – |
| 08 | OCR-Prüfung | **n. z.** | Leo verarbeitet Word-Dateien, keine Scans. | – |
| 09 | Passende Tags | **teilweise** | Nachbearbeitung: Bereiche, Absätze und Überschriften, die LibreOffice in einen Absatz hängt (Word-Rahmen, Textfelder), werden herausgelöst (Basisprüfung „Falsch verschachtelte Struktur“). Vorbereitung: Beschriftungen werden Absätze statt frei stehender `Caption`; das Literaturverzeichnis wird Text statt `TOC`. Analyse: Hinweise auf fett gesetzte Schein-Überschriften, getippte Aufzählungen, leere Absätze als Abstand, einzeilige Tabellen (Layout). | **Lesereihenfolge** (mehrspaltig, Textfelder, schwebende Bilder) und ob die Tags die Bedeutung richtig wiedergeben. |
| 10 | Zeichenzuordnung (Unicode) | **teilweise** | Keine eigene Korrektur. LibreOffice bettet die Schriften mit Unicode-Zuordnung ein. veraPDF prüft das. | Bei Auffälligkeiten: Schrift installieren (siehe INSTALL.md, Schriften). |
| 11 | Natürliche Sprache | **teilweise** | Analyse: Hauptsprache wird abgefragt und gesetzt. Basisprüfung „Dokumentsprache fehlt“. Sprachwechsel im Text übernimmt LibreOffice nur, wenn sie in Word gesetzt sind. | **Fremdsprachige Passagen** in Word kennzeichnen. Leo erkennt sie nicht. |
| 12 | Dehnbare Zeichen | **n. z.** | Betrifft Schriftsysteme wie Arabisch oder Japanisch. | – |
| 13 | Grafiken | **teilweise** | Analyse: fehlende und nichtssagende Alternativtexte („Bild 3“, Dateiname), Übernahme der Bildunterschrift, „dekorativ“. Hinweise auf Vektorgrafiken und SmartArt mit Text und auf als Bild eingefügte Tabellen. Vorbereitung: Grafiken in Kopf-/Fußzeilen werden dekorativ. Basisprüfung „Abbildungen ohne Alternativtext“. | **Qualität der Alternativtexte.** Text in Grafiken (Bild statt echter Tabelle oder echtem Text). Gruppen von Grafiken, die nur zusammen Sinn ergeben. |
| 14 | Überschriften | **ja** | Analyse: übersprungene Ebenen werden per Klick korrigiert. Hinweise: keine Überschriften, erste Überschrift nicht Ebene 1, fett statt Überschrift. Basisprüfung „Überschriftenebene übersprungen“, „Keine Überschriften“. | Ist alles, was wie eine Überschrift aussieht, auch eine? |
| 15 | Tabellen | **teilweise** | Analyse: erste Zeile als Kopfzeile markieren. Basisprüfung „Tabellen ohne Kopfzellen“, veraPDF. **Nicht:** Zeilenköpfe (erste Spalte), mehrstufige Köpfe, `Headers`/`IDs`. LibreOffice setzt an Kopfzellen `Scope /Column` und bei verbundenen Zellen `RowSpan`/`ColSpan`, auch bei zweistufigem Kopf (geprüft am Testbericht). | Einfache Tabellen bevorzugen. Komplexe Tabellen und Kopfspalten prüfen. Layout-Tabellen vermeiden. |
| 16 | Listen | **teilweise** | LibreOffice taggt Word-Aufzählungen als Liste, veraPDF prüft den Aufbau. Analyse: Hinweis auf getippte Aufzählungen. Im Test korrekt verschachtelt (`L`/`LI`/`Lbl`/`LBody`, auch Unterlisten). Schönheitsfehler: Aufzählungszeichen fehlen in `Lbl`, `ListNumbering` fehlt bei „a) b)“ und Aufzählungszeichen (verlangt erst PDF/UA-2). | Aufzählungen nur mit der Listenfunktion von Word. |
| 17 | Mathematische Ausdrücke | **teilweise** | Analyse: Formeln aus dem Formeleditor von Word werden aufgelistet, mit der Linearform als Vorschlag (z. B. `E=mc^2`) und eigenem Text. Nachbearbeitung: der Text wird als `Alt` an `Formula` gesetzt (nur wenn die Anzahl der Formeln in Word und PDF übereinstimmt). Basisprüfung „Formeln ohne Alternativtext“. Kein MathML (sieht erst PDF/UA-2 vor). | Wird der Text verständlich vorgelesen? Komplexe Formeln ausformulieren. |
| 18 | Kopf- und Fußzeilen | **ja** | LibreOffice setzt sie als Artefakte. Vorbereitung: Links und Verweis-Felder (`STYLEREF`, `PAGEREF` …) werden Text, Bilder und Formen dekorativ. | – |
| 19 | Fußnoten und Verweise | **teilweise** | Keine eigene Korrektur. LibreOffice taggt Fuß- und Endnoten als `Note` mit eindeutiger `ID`, mit Nummer (`Lbl`) und Link hin und zurück (geprüft am Testbericht und an den 6 Fußnoten des Praxistest-Berichts aus Word). veraPDF prüft die `ID`. Die Noten stehen in der Lesereihenfolge am Ende des Dokuments. | – |
| 20 | Optionale Inhalte (Ebenen) | **n. z.** | Kommen in Word-Berichten nicht vor. | – |
| 21 | Eingebettete Dateien | **n. z.** | LibreOffice bettet keine Dateien ein. | – |
| 22 | Artikelflüsse | **n. z.** | – | – |
| 23 | Digitale Signaturen | **n. z.** | Leo signiert nicht. | – |
| 24 | Nicht interaktive Formulare | **n. z.** | Formulare werden nicht unterstützt. | Formulare nicht mit Leo erstellen. |
| 25 | XFA | **n. z.** | – | – |
| 26 | Sicherheit | **n. z.** | LibreOffice verschlüsselt nicht; veraPDF prüft, ob Hilfstechnik Zugriff hat. | Das PDF nachträglich nicht mit Rechte-Einschränkungen versehen. |
| 27 | Navigation | **ja** | Überschriften werden Lesezeichen. Inhalts-, Abbildungs- und Tabellenverzeichnis bleiben verlinkt (LibreOffice 26.8). Basisprüfung „Keine Überschriften“. | – |
| 28 | Anmerkungen (Links) | **ja** | Analyse: fehlende Linkbeschreibungen werden aus dem Linktext ergänzt, Links auf lokale Dateien gemeldet; Hinweis auf nackte Webadressen. Vorbereitung: Links in Kopf-/Fußzeilen und auf lokale Dateien werden Text. Basisprüfung „Links ohne Tag“, „Links ohne Beschreibung“. | Sind die Linktexte verständlich? |
| 29 | Aktionen | **n. z.** | Kein JavaScript, keine Formular-Aktionen. | – |
| 30 | XObjects | **teilweise** | Keine eigene Korrektur. Bilder und Formen sind XObjects aus LibreOffice, veraPDF prüft sie. Referenz-XObjects kommen nicht vor. | – |
| 31 | Schriften | **ja** | Schriften sind im Container installiert (u. a. Carlito für Calibri). Basisprüfung „Nicht eingebettete Schriften“, veraPDF prüft den Rest. | Hausschriften ohne Ersatz (z. B. Aptos, Segoe UI) installieren, siehe INSTALL.md. |

## Zusammenfassung

| | Prüfpunkte |
|---|---|
| **ja**, abgedeckt | 02, 06, 07, 14, 18, 27, 28, 31 (8) |
| **teilweise** oder nur geprüft | 01, 04, 09, 10, 11, 13, 15, 16, 17, 19, 30 (11) |
| **nein**, nicht abgedeckt | – (0) |
| **n. z.**, nicht zutreffend für Word-Berichte | 03, 05, 08, 12, 20–26, 29 (12) |

**Ergebnis im Praxistest** (Praxistest-Abschlussbericht, 100 Seiten): veraPDF meldet nach
Vorbereitung und Korrekturen in Leo keine Verstöße mehr gegen PDF/UA-1, siehe
[FUNCTION.md](FUNCTION.md#praxistest-Praxistest-abschlussbericht-november-2026).

## Was Menschen immer prüfen müssen

Keine Software kann beurteilen:

1. ob die **Alternativtexte** den Inhalt und Zweck der Bilder wiedergeben,
2. ob die **Lesereihenfolge** stimmt, vor allem bei Textfeldern, Rahmen und mehrspaltigem Satz,
3. ob Information **nur über Farbe oder Lage** vermittelt wird,
4. ob **Tabellen** verständlich aufgebaut sind,
5. ob **fremdsprachige Passagen** gekennzeichnet sind,
6. ob **Linktexte** verständlich sind.

Leo verlangt für jedes Bild eine Entscheidung (Text oder dekorativ). Danach prüft man das
PDF abschließend mit PAC und am besten auch mit einem Screenreader (z. B. NVDA).

## Am Testbericht geprüft (November 2026)

Ein Testbericht mit Fußnoten, Endnote, Formel, mehrstufiger nummerierter Liste, Tabelle mit
zweistufigem Kopf und verbundenen Zellen und einem zweispaltigen Abschnitt ging durch Leo
(Analyse, Vorbereitung, LibreOffice 26.8, Nachbearbeitung):

| Fall | Ergebnis im PDF |
|---|---|
| Fuß- und Endnoten | `Note` mit eindeutiger `ID`, Nummer als `Lbl`, Link hin und zurück |
| Listen | `L`/`LI`/`Lbl`/`LBody`, Unterliste richtig verschachtelt |
| Tabelle | `TH` mit `Scope /Column`, `RowSpan`/`ColSpan` |
| Zwei Spalten | Lesereihenfolge richtig: linke Spalte, rechte Spalte, danach der Text darunter |
| Formel | vor der Formel-Unterstützung der einzige Verstoß (veraPDF 7.7-1), danach `Alt` gesetzt |

**veraPDF (PDF/UA-1): konform.**

Der Testbericht wurde mit LibreOffice erzeugt. Deshalb ging zusätzlich ein echter Bericht
aus Microsoft Word durch Leo.

## Praxistest mit dem Praxistest-Abschlussbericht (Word, November 2026)

100 Seiten, 64 Bilder, 12 Tabellen, 6 Fußnoten, 3 Word-Rahmen, keine Formeln. Durch Leo mit
allen Korrekturen, die man anklicken kann (die Alternativtexte der Bilder waren dabei
Platzhalter):

| Schritt | Ergebnis |
|---|---|
| Analyse | 64 Bilder, 12 Tabellen, 4 übersprungene Überschriftenebenen, 113 Links ohne Beschreibung |
| Vorbereitung | Vorlage „TD“ umbenannt, Beschriftungen als Absätze, Abstände in 424 Tabellenzellen, 8 Links in Kopf-/Fußzeilen, 10 Grafiken in Kopf-/Fußzeilen dekorativ |
| Nachbearbeitung | 3 Bereiche aus Absätzen gelöst, 7 dekorative Formen zu Artefakten |
| Fußnoten | alle 6 als `Note` mit eindeutiger `ID`, Nummer und Link |
| Basisprüfung | nichts offen |
| **veraPDF (PDF/UA-1)** | **konform** |

**Noch offen:** Formeln in einer Datei direkt aus Microsoft Word (der Praxistest-Bericht enthält keine).

## Ausblick: PDF/UA-2

Derselbe Praxistest-Bericht, zusätzlich mit veraPDF nach PDF/UA-2 (ISO 14289-2) geprüft:

| Regel | Anzahl | Was fehlt |
|---|---|---|
| 6.1-1, 8.2.5.2-2, 5-2, 5-5 | je 1 | Dateiversion PDF 2.0, Namensraum von PDF 2.0, Kennung „PDF/UA-2“ |
| 8.2.5.14-1 | 6 | Fußnoten heißen in PDF 2.0 `FENote` statt `Note` |
| 8.2.5.8-1 | 111 | Einträge in Verzeichnissen brauchen einen Verweis (`Ref`) auf ihr Ziel |
| 8.8-1 | 304 | Links im Dokument müssen auf das Strukturelement zeigen, nicht auf eine Stelle der Seite |
| 8.2.5.25-1 | 2 | `ListNumbering` an Listen |

Das sind fast alles Formatfragen von PDF 2.0, keine inhaltlichen Mängel. LibreOffice erzeugt
PDF 1.7 nach PDF/UA-1. PDF/UA-2 setzt voraus, dass LibreOffice PDF 2.0 schreibt oder Leo die
Datei umfassend umbaut. Deshalb ist PDF/UA-1 derzeit das Ziel von Leo. Es ist auch das, was
PAC und die meisten Prüfstellen erwarten.

## Die Oberfläche von Leo

Leo selbst soll barrierefrei bedienbar sein: beschriftete Formularfelder, Fehlermeldungen
als `role="alert"`, Statusmeldungen über `aria-live`, sichtbarer Fokus, ausreichende Kontraste
(auch im dunklen Modus). Die Reiter folgen dem ARIA-Muster „Tabs“ und lassen sich mit
Pfeil links/rechts, Pos1 und Ende bedienen.

Noch nicht systematisch getestet: Screenreader (NVDA, JAWS, VoiceOver) und Vergrößerung auf
200–400 %.
