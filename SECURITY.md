# Sicherheit · Security

**Deutsch** · [English](#english)

## Sicherheitslücke melden

Bitte melde Sicherheitslücken **nicht** als öffentliches Issue, sondern vertraulich über
GitHub: im Repository unter **Security → Report a vulnerability**
(„Private vulnerability reporting“). Die Meldung sehen nur die Betreuer des Projekts.

Hilfreich sind:

- was betroffen ist (z. B. Anmeldung, Upload, Export, Admin-Seite) und welche Version bzw. welcher Commit,
- Schritte zum Nachstellen, gern mit einer Beispieldatei ohne echte Inhalte,
- was ein Angreifer damit erreichen kann.

Wir bestätigen den Eingang innerhalb von 7 Tagen und melden uns, sobald wir die Lücke
eingeschätzt haben. Mimi wird nebenher an einer Schule betreut – größere Korrekturen können
etwas dauern. Nach der Korrektur nennen wir dich gern in den Release-Notizen, wenn du das möchtest.

## Unterstützte Versionen

Es gibt keine Versionszweige. Korrekturen kommen nur in den aktuellen Stand von `main`.

## Was dazugehört

- der Code in diesem Repository (Server, Weboberfläche, Bearbeitung von DOCX/ODT, PDF-Nachbearbeitung),
- die Vorlagen unter `deploy/`.

Lücken in **LibreOffice**, **veraPDF** oder anderen eingesetzten Programmen und Bibliotheken
bitte direkt dort melden. Wenn Mimi sie auf eine besondere Art auslöst oder verschlimmert, gern
zusätzlich hier.

## Bitte beim Testen

- Teste mit einer eigenen Installation (siehe [README.de.md](README.de.md)), nicht an fremden Servern.
- Keine Last- oder Überlastungstests, kein Zugriff auf Berichte oder Konten anderer.
- Wer sich daran hält und eine Lücke vertraulich meldet, hat von uns nichts zu befürchten.

## Schutzmaßnahmen im Überblick

Passwörter mit Argon2id gehasht, Benutzerdatei mit Rechten 600, freiwillige Zwei-Faktor-Anmeldung
(TOTP), Sitzungs-Cookie `HttpOnly` und `SameSite=Strict`, Origin-Prüfung bei schreibenden Anfragen,
Sperre nach Fehlversuchen, Audit-Log, automatisches Löschen der Berichte. Details und bekannte
Grenzen: [README.de.md](README.de.md).

---

<a id="english"></a>

## English

### Reporting a vulnerability

Please do **not** open a public issue. Report vulnerabilities privately via GitHub:
**Security → Report a vulnerability** in this repository (“Private vulnerability reporting”).
Only the project maintainers can see the report.

Helpful details:

- what is affected (e.g. login, upload, export, admin page) and which version or commit,
- steps to reproduce, ideally with a sample file without real content,
- what an attacker could achieve.

We acknowledge reports within 7 days and get back to you once we have assessed the issue.
Mimi is maintained alongside school work, so larger fixes may take a while. If you like, we
credit you in the release notes after the fix.

### Supported versions

There are no release branches. Fixes go into the current state of `main` only.

### Scope

- the code in this repository (server, web interface, DOCX/ODT processing, PDF post-processing),
- the templates under `deploy/`.

Please report vulnerabilities in **LibreOffice**, **veraPDF** or other programs and libraries
used directly to their projects. If Mimi triggers or worsens them in a particular way, feel free
to report here as well.

### When testing

- Test against your own installation (see [README.md](README.md)), not against other people's servers.
- No load or denial-of-service tests, no access to other users' reports or accounts.
- If you follow these rules and report privately, you have nothing to fear from us.

### Security measures at a glance

Passwords hashed with Argon2id, user file with mode 600, optional two-factor login (TOTP),
session cookie `HttpOnly` and `SameSite=Strict`, origin check on write requests, lockout after
failed attempts, audit log, automatic deletion of reports. Details and known limits:
[README.md](README.md).
