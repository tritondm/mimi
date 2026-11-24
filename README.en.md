# Rampe – making Word reports accessible (prototype)

[Deutsch](README.md) · **English**

Upload a Word report (.docx) → see problems and fix them right away → create and check an
accessible PDF (PDF/UA) → download the PDF, the corrected DOCX and an ODT.

- **User guide:** [INSTRUCTION.en.md](INSTRUCTION.en.md) (also as [PDF](INSTRUCTION.en.pdf))
- **How it works:** [FUNCTION.en.md](FUNCTION.en.md) (workflow, preparation for LibreOffice, field test)
- **Server installation, fonts, operation:** [INSTALL.en.md](INSTALL.en.md)

The user interface is in German.

## Getting started

```bash
cargo build --release
# once: create the first admin (password at least 12 characters, prompted without echo)
./target/release/rampe benutzer-anlegen chef --admin
./target/release/rampe
```

Then open <http://127.0.0.1:8080> in the browser and log in.

## Users and login

- Without logging in, nothing but the login page is reachable.
- Every user sees **only their own** uploaded reports.
- **Admins** manage users in the browser under **`/admin`**: create, set password,
  grant/revoke admin rights, delete. Admins cannot delete themselves or revoke their own
  admin rights, so there is always an admin left.
- Alternatively on the command line:
  `rampe benutzer-anlegen NAME [--admin]` (create user), `rampe passwort-ändern NAME`
  (change password, role is kept), `rampe benutzer-entfernen NAME` (remove user),
  `rampe benutzer-liste` (list users).
- Passwords are stored hashed with Argon2id in `users.txt` (file mode 600).
- Sessions: cookie `HttpOnly`, `SameSite=Strict`, 8 hours from the last activity.
  Changing a password or deleting a user ends that user's sessions immediately.
- After 5 failed attempts the user name is locked for 5 minutes.
- Write requests from foreign websites are rejected (origin check).
- Behind a reverse proxy with HTTPS, set `COOKIE_SECURE=1`.

## Settings (environment variables)

| Variable        | Default           | Meaning                                      |
|-----------------|-------------------|----------------------------------------------|
| `ADDR`          | `127.0.0.1:8080`  | address/port (for the network: `0.0.0.0:8080`) |
| `DATA_DIR`      | `./data`          | storage for uploaded reports                 |
| `USERS_FILE`    | `./users.txt`     | user file                                    |
| `COOKIE_SECURE` | –                 | `1` = cookie over HTTPS only                 |
| `SOFFICE`       | `soffice`         | path to LibreOffice                          |
| `VERAPDF`       | –                 | path to the veraPDF launcher script          |

Required on the server: **LibreOffice** (runs invisibly in the background) and the matching
**fonts** (above all Carlito as a substitute for Calibri), see [INSTALL.en.md](INSTALL.en.md#fonts).
Staff only need a browser.

## What the tool checks and fixes

| Area           | Check                                           | Fixable in the tool |
|----------------|-------------------------------------------------|---------------------|
| Images         | missing or meaningless alt text                 | yes (text field, use caption, “decorative”) |
| Tables         | first row not marked as header row              | yes (checkbox)      |
| Document       | title, main language                            | yes                 |
| Links          | missing link description (mainly tables of contents) | yes (automatic) |
| Headings       | skipped level (e.g. level 4 after level 2)      | yes (style of the correct level) |
| Font colours   | contrast below 4.5 : 1 against white (styles, bullets, direct formatting) | yes (darken shade) |
| Structure      | no headings, level jumps, bold instead of heading, typed lists, empty paragraphs as spacing | no – hint, fix in Word |

During PDF export Rampe automatically adjusts a **copy** of the document (the downloadable
DOCX stays unchanged):

- Custom styles named like a standard PDF tag (e.g. “TD”, “TH”, “Quote”) are renamed.
  Otherwise LibreOffice would map them to `P` via RoleMap – which would mis-tag e.g.
  **all table cells** (veraPDF 7.1-7, 7.2-10, 7.2-43).
- Links and reference fields (e.g. `STYLEREF`) in headers/footers become plain text,
  otherwise there is an untagged link on every page (veraPDF 7.18.5).
- Paragraph spacing from table styles is applied the way Word does it, otherwise table rows
  are taller in LibreOffice.
- Links to local files (`C:\Users\…`, `file://`) become text.
- Images and shapes in headers/footers become decorative (artifact instead of figure).
- The style “Beschriftung” (Word: `caption`) gets its own name. LibreOffice would otherwise
  tag captions as free-standing `Caption`, which PAC complains about; this way they become
  normal paragraphs. Look and numbering stay the same.
- Bibliographies (Word `BIBLIOGRAPHY`, Citavi, Zotero, EndNote) become normal text.
  LibreOffice would otherwise turn them into a protected index with different formatting,
  tagged as a table of contents (`TOC`).

Images are embedded at full resolution with JPEG quality 95. Optionally this can be done
losslessly via a checkbox before export: better quality and no fringes on transparent
backgrounds, but a considerably larger file.

After export Rampe fixes the PDF's structure tree: contents of Word frames and text boxes
(title block, imprint) that LibreOffice puts inside a paragraph are moved out of it. The
layout stays unchanged, see [FUNCTION.en.md](FUNCTION.en.md#post-processing-of-the-pdf).

After export the tool checks the PDF:

1. **Basic check** (always): tagged, language, title + title display, PDF/UA identifier,
   remapped standard tags, alt texts, headings and level jumps, nesting,
   table headers, embedded fonts, link descriptions, untagged links.
2. **veraPDF** (optional, recommended): complete automated check against PDF/UA-1
   (Matterhorn Protocol – the same basis as PAC).

The final test with **PAC** remains with the staff member (PAC is Windows-only and has no
command line). After Rampe it should only be a formality.

## Installing veraPDF (once, recommended)

Download veraPDF (Java) from <https://verapdf.org/software/>, run the installer and choose
`tools/verapdf` (in this project) as the target folder:

```bash
java -jar verapdf-izpack-installer-*.jar
```

Rampe then finds veraPDF automatically (or set the path with `VERAPDF=...`).
On the server: see [INSTALL.en.md](INSTALL.en.md#4-verapdf).

## Known limitations of the prototype

- Uploaded files remain in `data/jobs/` (no automatic deletion yet).
- No built-in HTTPS – for network use put a reverse proxy (nginx/Caddy) in front.
- The lockout after failed attempts is per user name; an attacker could temporarily lock
  out a user this way.
- Images in headers/footers are not checked (LibreOffice marks headers/footers in the PDF
  as artifacts anyway).
- Old VML graphics (very old Word files) are not detected.
- Preview only for PNG/JPG/GIF/SVG, not for EMF/WMF.
- Font colours inside inserted graphics (EMF/WMF, images) are not checked; PAC does check
  text in vector graphics, though. Rampe points out EMF/WMF and SmartArt graphics with text.
- Contrast is only checked against white, not against shaded paragraphs/cells.
- Reference fields in headers/footers are only recognised as simple fields (`w:fldSimple`),
  not as complex fields.
- Free editing of the text is not supported; Collabora Online could be integrated for that
  later.
- The corrected DOCX has been tested with LibreOffice, not yet with Microsoft Word.

## Licence

Copyright (c) 2026 example.org

This program is licensed under the **European Union Public Licence (EUPL) v. 1.2** – see `LICENSE`.
The EUPL is equally valid in all official languages of the EU; the English version is available at
<https://interoperable-europe.ec.europa.eu/collection/eupl/eupl-text-eupl-12>.

The libraries used and their licences are listed in `THIRD-PARTY-LICENSES.md`.
