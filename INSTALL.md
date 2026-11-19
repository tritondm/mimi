# Installation und Betrieb

Wie Rampe intern arbeitet: [FUNCTION.md](FUNCTION.md) · Bedienung und Einstellungen: [README.md](README.md)

## Überblick Server

```
Internet ──https──▶ Host server.example.org (LXD)
                     ├─ Container rproxy   nginx + Let's Encrypt, rampe.example.org → CONTAINER-IP:8080
                     └─ Container rampe    Ubuntu 26.04: Rampe (systemd), LibreOffice, veraPDF, Schriften
```

| Was | Wo (im Container `rampe`) |
|---|---|
| Programm | `/opt/rampe/rampe` (statisch gebaut, musl) |
| Dienst | `/etc/systemd/system/rampe.service` (Vorlage: `deploy/rampe.service`) |
| Benutzer | Systembenutzer `rampe`, Home `/var/lib/rampe` |
| Benutzerdatei | `/var/lib/rampe/users.txt` (Rechte 600) |
| Berichte | `/var/lib/rampe/data/jobs/` |
| LibreOffice-Profil | `/var/lib/rampe/data/lo-profile` (legt Rampe selbst an) |
| veraPDF | `/opt/verapdf/verapdf` |
| nginx-Konfiguration | Container `rproxy`: `/etc/nginx/conf.d/rampe.conf` (Vorlage: `deploy/nginx-rampe.conf`) |

Stand der eingesetzten Versionen (November 2026): Ubuntu 26.04.1 LTS, LibreOffice 26.2.6,
OpenJDK 25 (headless), veraPDF 1.30.

## Neuen Server einrichten

Alle Befehle als `root` auf dem Host.

### 1. Container anlegen

```bash
lxc launch ubuntu:26.04 rampe
lxc config set rampe limits.cpu 2
lxc config set rampe limits.memory 1536MiB
```

So ist der Container derzeit eingestellt. LibreOffice läuft immer nur einmal gleichzeitig,
Rampe stellt weitere Exporte hinten an.

### 2. Pakete

```bash
lxc exec rampe -- apt-get update
lxc exec rampe -- apt-get install -y --no-install-recommends \
    libreoffice-writer-nogui default-jre-headless curl unzip \
    fonts-crosextra-carlito fonts-crosextra-caladea \
    fonts-liberation fonts-liberation2 fonts-dejavu fonts-opensymbol
```

`libreoffice-writer-nogui` ist LibreOffice ohne Oberfläche (kein X11 nötig).
Zu den Schriften siehe [unten](#schriften).

### 3. Benutzer und Verzeichnisse

```bash
lxc exec rampe -- useradd --system --home-dir /var/lib/rampe --create-home --shell /usr/sbin/nologin rampe
lxc exec rampe -- install -d -o rampe -g rampe -m 700 /var/lib/rampe/data
lxc exec rampe -- install -d -m 755 /opt/rampe
```

### 4. veraPDF

```bash
lxc exec rampe -- sh -c 'cd /tmp && curl -fsSLO https://software.verapdf.org/releases/verapdf-installer.zip && unzip -q verapdf-installer.zip'
lxc file push deploy/verapdf-auto-install.xml rampe/tmp/verapdf-auto-install.xml
lxc exec rampe -- sh -c 'java -jar /tmp/verapdf-greenfield-*/verapdf-izpack-installer-*.jar /tmp/verapdf-auto-install.xml && rm -rf /tmp/verapdf*'
lxc exec rampe -- /opt/verapdf/verapdf --version
```

`deploy/verapdf-auto-install.xml` installiert ohne Rückfragen nach `/opt/verapdf`.
Der Dienst findet es über `VERAPDF=/opt/verapdf/verapdf`.

### 5. Programm aufspielen

Auf dem Entwicklungsrechner (einmalig `rustup target add x86_64-unknown-linux-musl`):

```bash
deploy/deploy.sh                       # Standard: root@server.example.org, Container rampe
deploy/deploy.sh root@anderer-host ct  # anderer Host / Container
```

Das Skript baut das statische Programm und kopiert Programm und Dienstdatei in den Container.
Danach startet es den Dienst neu und prüft, ob er im Container per HTTP antwortet.

### 6. Ersten Admin anlegen

Das Passwort wird verdeckt abgefragt (mindestens 12 Zeichen):

```bash
lxc exec rampe -- runuser -u rampe -- env USERS_FILE=/var/lib/rampe/users.txt /opt/rampe/rampe benutzer-anlegen chef --admin
lxc exec rampe -- systemctl restart rampe
```

Weitere Benutzer legt der Admin im Browser unter `/admin` an.
Ohne Admin startet der Dienst nicht. Im Journal steht dann der Hinweis auf diesen Befehl.

### 7. nginx (Container `rproxy`)

```bash
lxc file push deploy/nginx-rampe.conf rproxy/etc/nginx/conf.d/rampe.conf
lxc exec rproxy -- certbot certonly --nginx -d rampe.example.org   # nur beim ersten Mal
lxc exec rproxy -- sh -c 'nginx -t && systemctl reload nginx'
```

Wichtig in der Konfiguration: `client_max_body_size 300m` (große Berichte) und
`proxy_read_timeout 300s` (Export und Prüfung großer Berichte dauern eine Weile).
Die IP in `upstream` muss zur Adresse des Containers passen (`lxc list rampe`).

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
nicht nötig. Im Praxistest-Bericht landet der gesamte Fließtext als Carlito im PDF.

**Ohne gleichwertigen Ersatz:** Corbel, Tahoma, Consolas, Segoe UI, Aptos (neue
Standardschrift ab Word 2024). Kommen sie im Fließtext vor, weicht das Layout ab.

**Prüfen**, welche Schriften installiert sind und welche ein PDF tatsächlich verwendet:

```bash
lxc exec rampe -- fc-list : family | sort -u
lxc exec rampe -- fc-match Calibri            # sollte "Carlito" liefern
pdffonts bericht.pdf                          # auf dem eigenen Rechner (poppler-utils)
```

Auch die Basisprüfung von Rampe listet die eingebetteten Schriften auf.

**Eigene Schriften nachinstallieren** (z. B. die Original-Schriften der Hausvorlage, sofern die
Lizenz den Einsatz auf einem Server erlaubt; Microsoft-Schriften aus Windows oder Office sind
in der Regel **nicht** dafür lizenziert):

```bash
lxc exec rampe -- install -d /usr/local/share/fonts/rampe
lxc file push Schrift-Regular.ttf Schrift-Bold.ttf rampe/usr/local/share/fonts/rampe/
lxc exec rampe -- fc-cache -f
lxc exec rampe -- systemctl restart rampe
```

## Entwicklungsrechner

```bash
cargo build --release
./target/release/rampe benutzer-anlegen chef --admin
./target/release/rampe                       # http://127.0.0.1:8080
```

Benötigt LibreOffice (`soffice` im PATH oder `SOFFICE=...`). Für gleiche Ergebnisse wie am
Server dieselben Schriften installieren, unter Arch Linux z. B. `pacman -S ttf-carlito ttf-caladea`.
Ohne Carlito wird Calibri hier durch Liberation Sans ersetzt, und das Layout weicht ab.

veraPDF lokal: Installer von <https://verapdf.org/software/> nach `tools/verapdf` (wird von
Rampe automatisch gefunden) oder an einen beliebigen Ort mit Verweis im PATH bzw. `VERAPDF=...`.

## Betrieb

```bash
lxc exec rampe -- systemctl status rampe
lxc exec rampe -- journalctl -u rampe -f            # Protokoll
lxc exec rampe -- apt-get upgrade                   # LibreOffice-Updates kommen über Ubuntu
```

- Nach einem LibreOffice-Update `cargo test --release -- --ignored` laufen lassen
  (Regressionstest, siehe [FUNCTION.md](FUNCTION.md#testen)). Dazu einen echten Bericht
  exportieren und mit veraPDF prüfen. Die Vorbereitung (`prepare_for_pdf`) und die
  Nachbearbeitung (`pdffix`) gleichen Eigenheiten von LibreOffice aus.
- Hochgeladene Berichte werden noch nicht automatisch gelöscht (`/var/lib/rampe/data/jobs/`).
- Sicherung: `/var/lib/rampe/users.txt` genügt. Berichte lassen sich jederzeit neu hochladen.
