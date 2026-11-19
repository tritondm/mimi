# Rampe – Word-Berichte barrierefrei machen (Prototyp)

Word-Bericht (.docx) hochladen → Probleme anzeigen und direkt beheben → barrierefreies
PDF (PDF/UA) erzeugen und prüfen → PDF, korrigierte DOCX und ODT herunterladen.

## Starten

```bash
cargo build --release
# einmalig: ersten Admin anlegen (Passwort mind. 12 Zeichen, wird verdeckt abgefragt)
./target/release/rampe benutzer-anlegen chef --admin
./target/release/rampe
```

Dann im Browser öffnen: <http://127.0.0.1:8080> und anmelden.

## Benutzer und Anmeldung

- Ohne Anmeldung ist nichts außer der Anmeldeseite erreichbar.
- Jeder Benutzer sieht **nur seine eigenen** hochgeladenen Berichte.
- **Admins** verwalten Benutzer im Browser unter **`/admin`**: anlegen, Passwort setzen,
  Admin-Rechte vergeben/entziehen, löschen. Sich selbst löschen oder die eigenen
  Admin-Rechte entziehen geht nicht – es bleibt also immer ein Admin.
- Alternativ über die Kommandozeile:
  `rampe benutzer-anlegen NAME [--admin]`, `rampe benutzer-entfernen NAME`, `rampe benutzer-liste`.
- Passwörter werden mit Argon2id gehasht in `users.txt` gespeichert (Dateirechte 600).
- Sitzungen: Cookie `HttpOnly`, `SameSite=Strict`, 8 Stunden ab letzter Aktivität.
  Passwortänderung oder Löschung beendet die Sitzungen des Benutzers sofort.
- Nach 5 Fehlversuchen wird der Benutzername 5 Minuten gesperrt.
- Schreibende Anfragen von fremden Webseiten werden abgewiesen (Origin-Prüfung).
- Hinter einem Reverse-Proxy mit HTTPS `COOKIE_SECURE=1` setzen.

## Einstellungen (Umgebungsvariablen)

| Variable        | Standard          | Bedeutung                                   |
|-----------------|-------------------|---------------------------------------------|
| `ADDR`          | `127.0.0.1:8080`  | Adresse/Port (für das Netz: `0.0.0.0:8080`) |
| `DATA_DIR`      | `./data`          | Ablage der hochgeladenen Berichte           |
| `USERS_FILE`    | `./users.txt`     | Benutzerdatei                               |
| `COOKIE_SECURE` | –                 | `1` = Cookie nur über HTTPS                 |
| `SOFFICE`       | `soffice`         | Pfad zu LibreOffice                         |
| `VERAPDF`       | –                 | Pfad zum veraPDF-Startskript                |

Voraussetzung auf dem Server: **LibreOffice** (läuft unsichtbar im Hintergrund).
Die Mitarbeitenden brauchen nur einen Browser.

## Was das Tool prüft und behebt

| Bereich        | Prüfung                                         | Im Tool behebbar |
|----------------|-------------------------------------------------|------------------|
| Bilder         | fehlender oder nichtssagender Alternativtext    | ja (Textfeld, Bildunterschrift übernehmen, „dekorativ“) |
| Tabellen       | erste Zeile nicht als Kopfzeile markiert        | ja (Häkchen)     |
| Dokument       | Titel, Hauptsprache                             | ja               |
| Links          | fehlende Linkbeschreibung (v. a. Verzeichnisse) | ja (automatisch) |
| Überschriften  | übersprungene Ebene (z. B. Ebene 4 nach Ebene 2) | ja (Vorlage der richtigen Ebene) |
| Schriftfarben  | Kontrast unter 4,5 : 1 zu Weiß (Vorlagen, Aufzählungszeichen, direkt) | ja (Farbton abdunkeln) |
| Struktur       | keine Überschriften, Sprünge in den Ebenen, fett statt Überschrift, getippte Aufzählungen, leere Absätze als Abstand | nein – Hinweis, in Word korrigieren |

Beim PDF-Export passt Rampe eine **Kopie** des Dokuments automatisch an (die herunterladbare
DOCX bleibt unverändert):

- Eigene Formatvorlagen, die wie ein PDF-Standard-Tag heißen (z. B. „TD“, „TH“, „Quote“),
  werden umbenannt. LibreOffice würde sie sonst per RoleMap auf `P` umleiten – damit wären
  z. B. **alle Tabellenzellen** falsch getaggt (veraPDF 7.1-7, 7.2-10, 7.2-43).
- Links und Verweis-Felder (z. B. `STYLEREF`) in Kopf-/Fußzeilen werden zu normalem Text,
  sonst entsteht auf jeder Seite ein ungetaggter Link (veraPDF 7.18.5).

Nach dem Export prüft das Tool das PDF:

1. **Basisprüfung** (immer): getaggt, Sprache, Titel + Anzeige des Titels, PDF/UA-Kennung,
   umgeleitete Standard-Tags, Alternativtexte, Überschriften und Ebenensprünge, Tabellenköpfe,
   eingebettete Schriften, Linkbeschreibungen, ungetaggte Links.
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

Rampe findet veraPDF dann automatisch (oder Pfad über `VERAPDF=...` angeben).

## Bekannte Grenzen des Prototyps

- Hochgeladene Dateien bleiben in `data/jobs/` liegen (noch kein automatisches Löschen).
- Kein HTTPS eingebaut – für den Netzbetrieb einen Reverse-Proxy (nginx/Caddy) vorschalten.
- Die Sperre nach Fehlversuchen gilt pro Benutzername; ein Angreifer könnte so einen Benutzer
  vorübergehend aussperren.
- Bilder in Kopf-/Fußzeilen werden nicht geprüft (LibreOffice blendet Kopf-/Fußzeilen im PDF
  ohnehin als Artefakt aus).
- Alte VML-Grafiken (sehr alte Word-Dateien) werden nicht erkannt.
- Vorschau nur für PNG/JPG/GIF/SVG, nicht für EMF/WMF.
- Schriftfarben in eingefügten Grafiken (EMF/WMF, Bilder) werden nicht geprüft; PAC prüft
  Text in Vektorgrafiken aber mit.
- Kontrast wird nur gegen Weiß geprüft, nicht gegen farbig hinterlegte Absätze/Zellen.
- Verweis-Felder in Kopf-/Fußzeilen werden nur als einfaches Feld (`w:fldSimple`) erkannt,
  nicht als zusammengesetztes Feld.
- Freies Bearbeiten des Textes ist nicht vorgesehen; dafür könnte später Collabora Online
  eingebunden werden.
- Die korrigierte DOCX wurde mit LibreOffice getestet, noch nicht mit Microsoft Word.

## Lizenz

Copyright (c) 2026 example.org

Dieses Programm steht unter der **European Union Public Licence (EUPL) v. 1.2** – siehe `LICENSE`.
Die EUPL ist in allen Amtssprachen der EU gleichermaßen gültig; die deutsche Fassung ist unter
<https://interoperable-europe.ec.europa.eu/collection/eupl/eupl-text-eupl-12> abrufbar.

Die verwendeten Bibliotheken und deren Lizenzen sind in `THIRD-PARTY-LICENSES.md` aufgeführt.
