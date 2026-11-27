# Mimi – Word-Berichte barrierefrei machen (Prototyp)

[English](README.md) · **Deutsch**

**Mimi** steht für **M**acht **I**nhalte **M**ühelos **I**nklusiv.

Word-Bericht (.docx) oder OpenDocument-Text (.odt) hochladen → Probleme anzeigen und direkt beheben → barrierefreies
PDF (PDF/UA) erzeugen und prüfen → PDF, korrigierte DOCX und ODT herunterladen.

- **Anleitung für Benutzer:** [INSTRUCTION.md](INSTRUCTION.md) (auch als [PDF](INSTRUCTION_de.pdf))
- **Matterhorn-Protokoll (PDF/UA-1), was Mimi abdeckt:** [MATTERHORN.md](MATTERHORN.md) (auch als [PDF](MATTERHORN_de.pdf))
- **Wie es funktioniert:** [FUNCTION.md](FUNCTION.md) (Ablauf, Vorbereitung für LibreOffice, Praxistest)
- **Installation am Server, Schriften, Betrieb:** [INSTALL.md](INSTALL.md)

## Starten

```bash
cargo build --release
# einmalig: ersten Admin anlegen (Passwort mind. 12 Zeichen, wird verdeckt abgefragt)
./target/release/mimi benutzer-anlegen chef --admin
./target/release/mimi
```

Dann im Browser öffnen: <http://127.0.0.1:8080> und anmelden.

## Benutzer und Anmeldung

- Ohne Anmeldung ist nichts außer der Anmeldeseite erreichbar.
- Jeder Benutzer sieht **nur seine eigenen** hochgeladenen Berichte.
- Berichte werden 30 Tage nach der letzten Nutzung automatisch gelöscht (`JOB_TTL_DAYS`),
  ältere Berichte eines Benutzers einen Tag, nachdem er einen neuen hochgeladen hat. Mit
  „Bericht jetzt löschen“ geht es sofort. Wird ein Benutzer gelöscht, verschwinden auch seine Berichte.
- **Admins** verwalten Benutzer im Browser unter **`/admin`**: anlegen, Passwort setzen,
  Admin-Rechte vergeben/entziehen, löschen. Sich selbst löschen oder die eigenen
  Admin-Rechte entziehen geht nicht – es bleibt also immer ein Admin.
- Alternativ über die Kommandozeile:
  `mimi benutzer-anlegen NAME [--admin]`, `mimi passwort-ändern NAME` (Rolle bleibt),
  `mimi benutzer-entfernen NAME`, `mimi benutzer-liste`,
  `mimi zwei-faktor-aus NAME` (Zwei-Faktor zurücksetzen).
- **Zwei-Faktor-Anmeldung (freiwillig):** Jeder kann sie über den Knopf „Zwei-Faktor“ mit
  einer Authenticator-App einrichten (TOTP nach RFC 6238: 6 Ziffern, 30 Sekunden). Danach
  fragt Mimi nach dem Passwort einen Code ab, jeder Code gilt nur einmal, ein falscher Code
  zählt als Fehlversuch. Handy verloren: Admins setzen sie unter `/admin` zurück.
- **Audit-Log:** Anmeldungen, Benutzerverwaltung, Zwei-Faktor und Aktionen an Berichten
  (hochgeladen, PDF erzeugt, heruntergeladen, gelöscht) mit Zeit, Benutzer und IP-Adresse.
  Ohne Dateinamen und Inhalte. Admins sehen es unter `/protokoll`, mit Filtern nach Zeitraum und Aktion. Einträge werden
  nach 90 Tagen gelöscht (`AUDIT_DAYS`).
- Passwörter werden mit Argon2id gehasht in `users.txt` gespeichert (Dateirechte 600),
  dort auch die Geheimnisse der Zwei-Faktor-Anmeldung.
- Sitzungen: Cookie `HttpOnly`, `SameSite=Strict`, 8 Stunden ab letzter Aktivität.
  Passwortänderung oder Löschung beendet die Sitzungen des Benutzers sofort.
- Nach 5 Fehlversuchen wird der Benutzername 5 Minuten gesperrt.
- Schreibende Anfragen von fremden Webseiten werden abgewiesen (Origin-Prüfung).
- Hochgeladene Dateien werden vor dem Entpacken geprüft: höchstens 256 MB je Teil, 1 GB
  insgesamt und 10.000 Teile (Schutz vor „ZIP-Bomben“). LibreOffice bekommt nur Kopien ohne
  verknüpfte externe Inhalte (Bilder, Vorlagen, Abschnitte per Link) und lädt so nichts aus dem
  Netz oder vom Server nach. Hyperlinks bleiben erhalten.
- Ein Bericht kann nicht gleichzeitig exportiert, geändert oder gelöscht werden (sonst 409).
  Bricht ein Export ab, wird LibreOffice mit beendet.
- Hinter einem Reverse-Proxy mit HTTPS `COOKIE_SECURE=1` setzen.
- Darstellung hell, dunkel oder automatisch (wie im Betriebssystem), umschaltbar in der Kopfzeile, gespeichert im Browser.
- Sprache Deutsch (Standard) oder Englisch, umschaltbar in der Kopfzeile, gespeichert im Browser (Cookie `mimi-lang`).
  Meldungen, Prüfergebnisse und der Name heruntergeladener Dateien folgen der Sprache; Server-Ausgaben und das
  Audit-Log bleiben deutsch (die Protokollseite übersetzt sie).

## Einstellungen (Umgebungsvariablen)

| Variable        | Standard          | Bedeutung                                   |
|-----------------|-------------------|---------------------------------------------|
| `ADDR`          | `127.0.0.1:8080`  | Adresse/Port (für das Netz: `0.0.0.0:8080`) |
| `DATA_DIR`      | `./data`          | Ablage der hochgeladenen Berichte           |
| `USERS_FILE`    | `./users.txt`     | Benutzerdatei                               |
| `COOKIE_SECURE` | –                 | `1` = Cookie nur über HTTPS                 |
| `SOFFICE`       | `soffice`         | Pfad zu LibreOffice                         |
| `VERAPDF`       | –                 | Pfad zum veraPDF-Startskript                |
| `JOB_TTL_DAYS`  | `30`              | Berichte so viele Tage nach der letzten Nutzung löschen, ersetzte nach 1 Tag; `0` = aus |
| `AUDIT_FILE`    | `<DATA_DIR>/audit.log` | Audit-Log (eine JSON-Zeile pro Ereignis) |
| `AUDIT_DAYS`    | `90`              | Einträge im Audit-Log so viele Tage aufbewahren; `0` = unbegrenzt |
| `DEFAULT_LANG`  | `de`              | Sprache der Oberfläche ohne Wahl im Browser: `de` oder `en` |

Voraussetzung auf dem Server: **LibreOffice** (läuft unsichtbar im Hintergrund) und die
passenden **Schriften** (v. a. Carlito als Ersatz für Calibri), siehe [INSTALL.md](INSTALL.md#schriften).
Die Mitarbeitenden brauchen nur einen Browser.

## Was das Tool prüft und behebt

| Bereich        | Prüfung                                         | Im Tool behebbar |
|----------------|-------------------------------------------------|------------------|
| Bilder         | fehlender oder nichtssagender Alternativtext    | ja (Textfeld, Bildunterschrift übernehmen, „dekorativ“) |
| Formeln        | Formeln aus dem Formeleditor brauchen einen Alternativtext | ja (Linearform als Vorschlag, eigener Text) |
| Tabellen       | erste Zeile nicht als Kopfzeile markiert        | ja (Häkchen)     |
| Dokument       | Titel, Hauptsprache                             | ja               |
| Links          | fehlende Linkbeschreibung (v. a. Verzeichnisse) | ja (automatisch) |
| Überschriften  | übersprungene Ebene (z. B. Ebene 4 nach Ebene 2) | ja (Vorlage der richtigen Ebene) |
| Schriftfarben  | Kontrast unter 4,5 : 1 zu Weiß (Vorlagen, Aufzählungszeichen, direkt) | ja (Farbton abdunkeln) |
| Struktur       | keine Überschriften, Sprünge in den Ebenen, fett statt Überschrift, getippte Aufzählungen, leere Absätze als Abstand | nein – Hinweis, in Word korrigieren |

Beim PDF-Export passt Mimi eine **Kopie** des Dokuments automatisch an (die herunterladbare
DOCX bleibt unverändert):

- Formatvorlagen, die wie ein PDF-Standard-Tag heißen (z. B. „TD“, „TH“, „Quote“),
  werden umbenannt. LibreOffice würde sie sonst per RoleMap auf `P` umleiten – damit wären
  z. B. **alle Tabellenzellen** falsch getaggt (veraPDF 7.1-7, 7.2-10, 7.2-43).
- Links und Verweis-Felder (z. B. `STYLEREF`) in Kopf-/Fußzeilen werden zu normalem Text,
  sonst entsteht auf jeder Seite ein ungetaggter Link (veraPDF 7.18.5).
- Absatzabstände aus Tabellenvorlagen werden wie in Word übernommen, sonst sind die
  Tabellenzeilen in LibreOffice höher.
- Links auf lokale Dateien (`C:\Users\…`, `file://`) werden zu Text.
- Bilder und Formen in Kopf-/Fußzeilen werden dekorativ (Artefakt statt Abbildung).

Bilder werden in voller Auflösung mit JPEG-Qualität 95 eingebettet. Wahlweise geht das auch
verlustfrei, per Häkchen vor dem Export: bessere Qualität und keine Säume bei transparentem
Hintergrund, aber deutlich größere Datei.
- Die Vorlage „Beschriftung“ (Word: `caption`) bekommt einen eigenen Namen. LibreOffice taggt
  Beschriftungen sonst als frei stehende `Caption`, was PAC bemängelt; so werden
  sie normale Absätze. Aussehen und Nummerierung bleiben gleich.
- Literaturverzeichnisse (Word `BIBLIOGRAPHY`, Citavi, Zotero, EndNote) werden zu normalem Text.
  LibreOffice macht daraus sonst ein geschütztes Verzeichnis mit anderer Formatierung, getaggt
  als Inhaltsverzeichnis (`TOC`).

Nach dem Export richtet Mimi den Strukturbaum des PDFs: Inhalte von Word-Rahmen und
Textfeldern (Titelblock, Impressum), die LibreOffice in einen Absatz hängt, werden dort
herausgelöst. Das Layout bleibt unverändert, siehe [FUNCTION.md](FUNCTION.md#nachbearbeitung-des-pdfs).

Nach dem Export prüft das Tool das PDF:

1. **Basisprüfung** (immer): getaggt, Sprache, Titel + Anzeige des Titels, PDF/UA-Kennung,
   umgeleitete Standard-Tags, Alternativtexte, Überschriften und Ebenensprünge, Verschachtelung,
   Tabellenköpfe, eingebettete Schriften, Linkbeschreibungen, ungetaggte Links.
2. **veraPDF** (optional, empfohlen): vollständige maschinelle Prüfung nach PDF/UA-1
   (Matterhorn-Protokoll – dieselbe Grundlage wie PAC).

Der abschließende Test mit **PAC** bleibt beim Mitarbeiter (PAC gibt es nur für Windows und
ohne Kommandozeile). Er sollte danach nur noch eine Formsache sein.

## veraPDF installieren (einmalig, empfohlen)

veraPDF (Java) von <https://verapdf.org/software/> herunterladen, den Installer starten und als
Zielordner `tools/verapdf` (in diesem Projekt) wählen:

```bash
java -jar verapdf-izpack-installer-*.jar
```

Mimi findet veraPDF dann automatisch (oder Pfad über `VERAPDF=...` angeben).
Am Server: siehe [INSTALL.md](INSTALL.md#4-verapdf).

## Bekannte Grenzen des Prototyps

- Kein HTTPS eingebaut – für den Netzbetrieb einen Reverse-Proxy (nginx/Caddy) vorschalten.
- Die Sperre nach Fehlversuchen gilt pro Benutzername; ein Angreifer könnte so einen Benutzer
  vorübergehend aussperren.
- Bilder in Kopf-/Fußzeilen werden nicht geprüft (LibreOffice blendet Kopf-/Fußzeilen im PDF
  ohnehin als Artefakt aus).
- Alte VML-Grafiken (sehr alte Word-Dateien) werden nicht erkannt.
- Vorschau nur für PNG/JPG/GIF/SVG, nicht für EMF/WMF.
- Schriftfarben in eingefügten Grafiken (EMF/WMF, Bilder) werden nicht geprüft; PAC prüft
  Text in Vektorgrafiken aber mit. Mimi weist auf EMF/WMF- und SmartArt-Grafiken mit Text hin.
- Kontrast wird nur gegen Weiß geprüft, nicht gegen farbig hinterlegte Absätze/Zellen.
- ODT-Dateien werden mit LibreOffice in eine Word-Fassung umgewandelt und so geprüft. Was
  diese Umwandlung nicht abbildet, kann im PDF abweichen.
- Freies Bearbeiten des Textes ist nicht vorgesehen; dafür könnte später Collabora Online
  eingebunden werden.
- Die korrigierte DOCX wurde mit LibreOffice getestet, noch nicht mit Microsoft Word.

## Lizenz

Copyright (c) 2026 Andreas Kraxner

Dieses Programm steht unter der **European Union Public Licence (EUPL) v. 1.2** – siehe `LICENSE`.
Die EUPL ist in allen Amtssprachen der EU gleichermaßen gültig; die deutsche Fassung ist unter
<https://interoperable-europe.ec.europa.eu/collection/eupl/eupl-text-eupl-12> abrufbar.

Die verwendeten Bibliotheken und deren Lizenzen sind in `THIRD-PARTY-LICENSES.md` aufgeführt.
