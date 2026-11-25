# Installation und Betrieb

*Mimi – Macht Inhalte Mühelos Inklusiv*

**Deutsch** · [English](INSTALL.en.md)

Wie Mimi intern arbeitet: [FUNCTION.md](FUNCTION.md) · Bedienung und Einstellungen: [README.md](README.md)

> **Technische Namen:** Programm und Befehl `mimi`, Container `mimi`, Dienst `mimi.service`,
> Systembenutzer und Pfade `/opt/mimi`, `/var/lib/mimi`.

## Überblick Server

```
Internet ──https──▶ Host (LXD)
                     ├─ Container rproxy   nginx + Let's Encrypt, eigene Adresse(n) → IP des Containers mimi, Port 8080
                     └─ Container mimi    Ubuntu 26.04: Mimi (systemd), LibreOffice, veraPDF, Schriften
```

| Was | Wo (im Container `mimi`) |
|---|---|
| Programm | `/opt/mimi/mimi` (statisch gebaut, musl) |
| Dienst | `/etc/systemd/system/mimi.service` (Vorlage: `deploy/mimi.service`) |
| Benutzer | Systembenutzer `mimi`, Home `/var/lib/mimi` |
| Benutzerdatei | `/var/lib/mimi/users.txt` (Rechte 600, mit den Geheimnissen der Zwei-Faktor-Anmeldung) |
| Audit-Log | `/var/lib/mimi/data/audit.log` (Rechte 600, 90 Tage) |
| Berichte | `/var/lib/mimi/data/jobs/` |
| LibreOffice-Profil | `/var/lib/mimi/data/lo-profile` (legt Mimi selbst an) |
| veraPDF | `/opt/verapdf/verapdf` |
| nginx-Konfiguration | Container `rproxy`: `/etc/nginx/conf.d/mimi.conf` (Vorlage: `deploy/nginx-mimi.conf`) |

Stand der eingesetzten Versionen (November 2026): Ubuntu 26.04.1 LTS, LibreOffice 26.8.3
(Pakete von The Document Foundation unter `/opt/libreoffice26.8`), OpenJDK 25 (headless), veraPDF 1.30.

## Neuen Server einrichten

Alle Befehle als `root` auf dem Host.

### 1. Container anlegen

```bash
lxc launch ubuntu:26.04 mimi
lxc config set mimi limits.cpu 2
lxc config set mimi limits.memory 1536MiB
```

So ist der Container derzeit eingestellt. LibreOffice läuft immer nur einmal gleichzeitig,
Mimi stellt weitere Exporte hinten an.

### 2. Pakete

```bash
lxc exec mimi -- apt-get update
lxc exec mimi -- apt-get install -y --no-install-recommends \
    libreoffice-writer-nogui default-jre-headless curl unzip \
    fonts-crosextra-carlito fonts-crosextra-caladea \
    fonts-liberation fonts-liberation2 fonts-dejavu fonts-opensymbol
```

`libreoffice-writer-nogui` ist LibreOffice ohne Oberfläche (kein X11 nötig).
Zu den Schriften siehe [unten](#schriften).

**LibreOffice 26.8 von The Document Foundation** (wird von Mimi verwendet): Das LibreOffice
26.2 aus Ubuntu 26.04 lässt beim PDF-Export alle Links aus Inhalts-, Abbildungs- und
Tabellenverzeichnis weg. Die offiziellen Pakete installieren sich zusätzlich nach `/opt`;
das Ubuntu-Paket bleibt als Rückfall und liefert die Abhängigkeiten.

```bash
lxc exec mimi -- sh -c '
  cd /root && curl -fsSLO https://download.documentfoundation.org/libreoffice/stable/26.8.3/deb/x86_64/LibreOffice_26.8.3_Linux_x86-64_deb.tar.gz
  tar xzf LibreOffice_26.8.3_Linux_x86-64_deb.tar.gz && apt-get install -y ./LibreOffice_26.8.3*_deb/DEBS/*.deb
  apt-get install -y libxinerama1 libcups2t64 libsm6 libice6 libx11-xcb1 libxrandr2
  /opt/libreoffice26.8/program/soffice --version'
```

`deploy/mimi.service` setzt `SOFFICE=/opt/libreoffice26.8/program/soffice`. Für eine neuere
Version Pfad und Versionsnummer anpassen.

### 3. Benutzer und Verzeichnisse

```bash
lxc exec mimi -- useradd --system --home-dir /var/lib/mimi --create-home --shell /usr/sbin/nologin mimi
lxc exec mimi -- install -d -o mimi -g mimi -m 700 /var/lib/mimi/data
lxc exec mimi -- install -d -m 755 /opt/mimi
```

### 4. veraPDF

```bash
lxc exec mimi -- sh -c 'cd /tmp && curl -fsSLO https://software.verapdf.org/releases/verapdf-installer.zip && unzip -q verapdf-installer.zip'
lxc file push deploy/verapdf-auto-install.xml mimi/tmp/verapdf-auto-install.xml
lxc exec mimi -- sh -c 'java -jar /tmp/verapdf-greenfield-*/verapdf-izpack-installer-*.jar /tmp/verapdf-auto-install.xml && rm -rf /tmp/verapdf*'
lxc exec mimi -- /opt/verapdf/verapdf --version
```

`deploy/verapdf-auto-install.xml` installiert ohne Rückfragen nach `/opt/verapdf`.
Der Dienst findet es über `VERAPDF=/opt/verapdf/verapdf`.

### 5. Programm aufspielen

Auf dem Entwicklungsrechner (einmalig `rustup target add x86_64-unknown-linux-musl`):

```bash
deploy/deploy.sh root@host              # Container mimi
deploy/deploy.sh root@host ct           # anderer Container
```

Den Host kann man auch einmal in `deploy/local/deploy.env` eintragen (`DEPLOY_HOST=root@host`),
dann genügt `deploy/deploy.sh`. Der Ordner `deploy/local/` gehört nicht ins Repository.

Das Skript baut das statische Programm und kopiert Programm und Dienstdatei in den Container.
Danach startet es den Dienst neu und prüft, ob er im Container per HTTP antwortet.

### 6. Ersten Admin anlegen

Das Passwort wird verdeckt abgefragt (mindestens 12 Zeichen):

```bash
lxc exec mimi -- runuser -u mimi -- env USERS_FILE=/var/lib/mimi/users.txt /opt/mimi/mimi benutzer-anlegen chef --admin
lxc exec mimi -- systemctl restart mimi
```

Weitere Benutzer legt der Admin im Browser unter `/admin` an.
Ohne Admin startet der Dienst nicht. Im Journal steht dann der Hinweis auf diesen Befehl.

### 7. nginx (Container `rproxy`)

```bash
lxc file push deploy/nginx-mimi.conf rproxy/etc/nginx/conf.d/mimi.conf
lxc exec rproxy -- certbot certonly --nginx --cert-name mimi.example.org -d mimi.example.org   # nur beim ersten Mal
lxc exec rproxy -- sh -c 'nginx -t && systemctl reload nginx'
```

Wichtig in der Konfiguration: `client_max_body_size 300m` (große Berichte) und
`proxy_read_timeout 300s` (Export und Prüfung großer Berichte dauern eine Weile).
In der Vorlage `mimi.example.org` durch die eigene Adresse ersetzen und in `upstream` die IP des
Containers eintragen (`lxc list mimi`). Mehrere Adressen gehen mit einem gemeinsamen Zertifikat
(weitere `-d` bei certbot, alle Namen in `server_name`). Für eine frühere Adresse enthält die Vorlage
einen Block mit dauerhafter Weiterleitung (301). Anmeldungen gelten pro Adresse, nach einem Wechsel
also einmal neu anmelden.

## Schriften

LibreOffice kann nur Schriften verwenden, die im Container installiert sind. Fehlt eine
Schrift, setzt es eine Ersatzschrift ein. Hat diese andere Zeichenbreiten, brechen Zeilen
anders um. Tabellenzeilen werden dann höher, und Seitenumbrüche verschieben sich.

**Installiert** (Paket → ersetzt in Word):

| Paket | Schrift | Ersetzt (gleiche Zeichenbreiten) |
|---|---|---|
| `fonts-crosextra-carlito` | Carlito | **Calibri** (Standardschrift seit Word 2007) |
| `fonts-crosextra-caladea` | Caladea | Cambria |
| `fonts-liberation`, `fonts-liberation2` | Liberation Sans / Serif / Mono | Arial, Times New Roman, Courier New |
| `fonts-dejavu` | DejaVu | allgemeine Ersatzschrift |
| `fonts-opensymbol` | OpenSymbol | Symbol, Wingdings (Aufzählungszeichen) |

LibreOffice ordnet Calibri → Carlito und Cambria → Caladea selbst zu, eine Einstellung ist
nicht nötig. Im Bericht aus dem Praxistest landet der gesamte Fließtext als Carlito im PDF.

**Ohne gleichwertigen Ersatz:** Corbel, Tahoma, Consolas, Segoe UI, Aptos (neue
Standardschrift ab Word 2024). Kommen sie im Fließtext vor, weicht das Layout ab.

**Prüfen**, welche Schriften installiert sind und welche ein PDF tatsächlich verwendet:

```bash
lxc exec mimi -- fc-list : family | sort -u
lxc exec mimi -- fc-match Calibri            # sollte "Carlito" liefern
pdffonts bericht.pdf                          # auf dem eigenen Rechner (poppler-utils)
```

Auch die Basisprüfung von Mimi listet die eingebetteten Schriften auf.

**Eigene Schriften nachinstallieren** (z. B. die Original-Schriften der Hausvorlage, sofern die
Lizenz den Einsatz auf einem Server erlaubt; Microsoft-Schriften aus Windows oder Office sind
in der Regel **nicht** dafür lizenziert):

```bash
lxc exec mimi -- install -d /usr/local/share/fonts/mimi
lxc file push Schrift-Regular.ttf Schrift-Bold.ttf mimi/usr/local/share/fonts/mimi/
lxc exec mimi -- fc-cache -f
lxc exec mimi -- systemctl restart mimi
```

## Entwicklungsrechner

```bash
cargo build --release
./target/release/mimi benutzer-anlegen chef --admin
./target/release/mimi                       # http://127.0.0.1:8080
```

Benötigt LibreOffice (`soffice` im PATH oder `SOFFICE=...`). Für gleiche Ergebnisse wie am
Server dieselben Schriften installieren, unter Arch Linux z. B. `pacman -S ttf-carlito ttf-caladea`.
Ohne Carlito wird Calibri hier durch Liberation Sans ersetzt, und das Layout weicht ab.

veraPDF lokal: Installer von <https://verapdf.org/software/> nach `tools/verapdf` (wird von
Mimi automatisch gefunden) oder an einen beliebigen Ort mit Verweis im PATH bzw. `VERAPDF=...`.

## Betrieb

```bash
lxc exec mimi -- systemctl status mimi
lxc exec mimi -- journalctl -u mimi -f            # Protokoll
lxc exec mimi -- apt-get upgrade                   # Systempakete; LibreOffice in /opt von Hand (siehe oben)
```

- Nach einem LibreOffice-Update `cargo test --release -- --ignored` laufen lassen
  (Regressionstest, siehe [FUNCTION.md](FUNCTION.md#testen)). Dazu einen echten Bericht
  exportieren und mit veraPDF prüfen. Die Vorbereitung (`prepare_for_pdf`) und die
  Nachbearbeitung (`pdffix`) gleichen Eigenheiten von LibreOffice aus.
- Berichte (`/var/lib/mimi/data/jobs/`) löscht Mimi automatisch: 30 Tage nach der letzten Nutzung
  (`JOB_TTL_DAYS` in `deploy/mimi.service`), ersetzte Berichte eines Benutzers nach 1 Tag.
  Mimi räumt beim Start und dann stündlich auf. Im Journal steht nur die Anzahl, z. B.
  „3 Bericht(e) gelöscht (412 MB, Löschfrist abgelaufen)“.
- Sicherung: `/var/lib/mimi/users.txt` genügt, dazu bei Bedarf `audit.log`. Berichte lassen sich jederzeit neu hochladen.
  `users.txt` enthält die Geheimnisse der Zwei-Faktor-Anmeldung: Sicherungen genauso schützen wie die Datei selbst.
- Zwei-Faktor für jemanden zurücksetzen, ohne Browser:
  `lxc exec mimi -- runuser -u mimi -- env USERS_FILE=/var/lib/mimi/users.txt DATA_DIR=/var/lib/mimi/data /opt/mimi/mimi zwei-faktor-aus NAME`
