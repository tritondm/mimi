# Installation and operation

*Mimi – Macht Inhalte Mühelos Inklusiv*

[Deutsch](INSTALL.md) · **English**

How Mimi works internally: [FUNCTION.en.md](FUNCTION.en.md) · Usage and settings: [README.en.md](README.en.md)

> **Technical names:** program and command `rampe`, container `rampe`, service `rampe.service`,
> system user and paths `/opt/rampe`, `/var/lib/rampe`.

## Server overview

```
Internet ──https──▶ Host server.example.org (LXD)
                     ├─ Container rproxy   nginx + Let's Encrypt, mimi.example.org + mimi.hblfa-tirol.at → CONTAINER-IP:8080
                     └─ Container rampe    Ubuntu 26.04: Mimi (systemd), LibreOffice, veraPDF, fonts
```

| What | Where (in the container `rampe`) |
|---|---|
| Program | `/opt/rampe/rampe` (statically built, musl) |
| Service | `/etc/systemd/system/rampe.service` (template: `deploy/rampe.service`) |
| User | system user `rampe`, home `/var/lib/rampe` |
| User file | `/var/lib/rampe/users.txt` (mode 600, with the two-factor secrets) |
| Audit log | `/var/lib/rampe/data/audit.log` (mode 600, 90 days) |
| Reports | `/var/lib/rampe/data/jobs/` |
| LibreOffice profile | `/var/lib/rampe/data/lo-profile` (created by Mimi itself) |
| veraPDF | `/opt/verapdf/verapdf` |
| nginx configuration | container `rproxy`: `/etc/nginx/conf.d/rampe.conf` (template: `deploy/nginx-rampe.conf`) |

Versions in use (November 2026): Ubuntu 26.04.1 LTS, LibreOffice 26.8.3
(packages from The Document Foundation under `/opt/libreoffice26.8`), OpenJDK 25 (headless), veraPDF 1.30.

## Setting up a new server

All commands as `root` on the host.

### 1. Create the container

```bash
lxc launch ubuntu:26.04 rampe
lxc config set rampe limits.cpu 2
lxc config set rampe limits.memory 1536MiB
```

This is how the container is currently configured. LibreOffice only ever runs once at a
time; Mimi queues further exports.

### 2. Packages

```bash
lxc exec rampe -- apt-get update
lxc exec rampe -- apt-get install -y --no-install-recommends \
    libreoffice-writer-nogui default-jre-headless curl unzip \
    fonts-crosextra-carlito fonts-crosextra-caladea \
    fonts-liberation fonts-liberation2 fonts-dejavu fonts-opensymbol
```

`libreoffice-writer-nogui` is LibreOffice without a user interface (no X11 needed).
For fonts see [below](#fonts).

**LibreOffice 26.8 from The Document Foundation** (used by Mimi): the LibreOffice
26.2 shipped with Ubuntu 26.04 drops all links from the table of contents and the lists of
figures and tables during PDF export. The official packages install additionally into
`/opt`; the Ubuntu package remains as a fallback and provides the dependencies.

```bash
lxc exec rampe -- sh -c '
  cd /root && curl -fsSLO https://download.documentfoundation.org/libreoffice/stable/26.8.3/deb/x86_64/LibreOffice_26.8.3_Linux_x86-64_deb.tar.gz
  tar xzf LibreOffice_26.8.3_Linux_x86-64_deb.tar.gz && apt-get install -y ./LibreOffice_26.8.3*_deb/DEBS/*.deb
  apt-get install -y libxinerama1 libcups2t64 libsm6 libice6 libx11-xcb1 libxrandr2
  /opt/libreoffice26.8/program/soffice --version'
```

`deploy/rampe.service` sets `SOFFICE=/opt/libreoffice26.8/program/soffice`. For a newer
version adjust the path and version number.

### 3. User and directories

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

`deploy/verapdf-auto-install.xml` installs into `/opt/verapdf` without prompts.
The service finds it via `VERAPDF=/opt/verapdf/verapdf`.

### 5. Deploy the program

On the development machine (once: `rustup target add x86_64-unknown-linux-musl`):

```bash
deploy/deploy.sh                       # default: root@server.example.org, container rampe
deploy/deploy.sh root@other-host ct    # other host / container
```

The script builds the static program and copies the program and the service file into the
container. It then restarts the service and checks whether it answers via HTTP inside the
container.

### 6. Create the first admin

The password is prompted without echo (at least 12 characters):

```bash
lxc exec rampe -- runuser -u rampe -- env USERS_FILE=/var/lib/rampe/users.txt /opt/rampe/rampe benutzer-anlegen chef --admin
lxc exec rampe -- systemctl restart rampe
```

The admin creates further users in the browser under `/admin`.
Without an admin the service does not start; the journal then points to this command.

### 7. nginx (container `rproxy`)

```bash
lxc file push deploy/nginx-rampe.conf rproxy/etc/nginx/conf.d/rampe.conf
lxc exec rproxy -- certbot certonly --nginx --cert-name mimi.example.org -d mimi.example.org -d mimi.hblfa-tirol.at   # first time only
lxc exec rproxy -- sh -c 'nginx -t && systemctl reload nginx'
```

Important in the configuration: `client_max_body_size 300m` (large reports) and
`proxy_read_timeout 300s` (export and check of large reports take a while).
The IP in `upstream` must match the container's address (`lxc list rampe`).
`mimi.example.org` and `mimi.hblfa-tirol.at` serve the same site (one certificate for both).
The old address `rampe.example.org` redirects permanently (301) to `mimi.example.org`; certbot keeps
renewing its certificate. Logins are per address, so users log in once more after the switch.

## Fonts

LibreOffice can only use fonts installed in the container. If a font is missing, it uses a
substitute. If the substitute has different character widths, lines break differently.
Table rows then get taller and page breaks shift.

**Installed** (package → replaces in Word):

| Package | Font | Replaces (same character widths) |
|---|---|---|
| `fonts-crosextra-carlito` | Carlito | **Calibri** (default font since Word 2007) |
| `fonts-crosextra-caladea` | Caladea | Cambria |
| `fonts-liberation`, `fonts-liberation2` | Liberation Sans / Serif / Mono | Arial, Times New Roman, Courier New |
| `fonts-dejavu` | DejaVu | general fallback font |
| `fonts-opensymbol` | OpenSymbol | Symbol, Wingdings (bullets) |

LibreOffice maps Calibri → Carlito and Cambria → Caladea by itself, no configuration is
needed. In the Praxistest report the entire body text ends up as Carlito in the PDF.

**Without an equivalent substitute:** Corbel, Tahoma, Consolas, Segoe UI, Aptos (new
default font from Word 2024). If they are used in body text, the layout differs.

**Check** which fonts are installed and which ones a PDF actually uses:

```bash
lxc exec rampe -- fc-list : family | sort -u
lxc exec rampe -- fc-match Calibri            # should return "Carlito"
pdffonts bericht.pdf                          # on your own machine (poppler-utils)
```

Mimi's basic check also lists the embedded fonts.

**Installing additional fonts** (e.g. the original fonts of the corporate template, provided
the licence allows use on a server; Microsoft fonts from Windows or Office are usually
**not** licensed for this):

```bash
lxc exec rampe -- install -d /usr/local/share/fonts/rampe
lxc file push Font-Regular.ttf Font-Bold.ttf rampe/usr/local/share/fonts/rampe/
lxc exec rampe -- fc-cache -f
lxc exec rampe -- systemctl restart rampe
```

## Development machine

```bash
cargo build --release
./target/release/rampe benutzer-anlegen chef --admin
./target/release/rampe                       # http://127.0.0.1:8080
```

Requires LibreOffice (`soffice` in PATH or `SOFFICE=...`). For the same results as on the
server install the same fonts, on Arch Linux e.g. `pacman -S ttf-carlito ttf-caladea`.
Without Carlito, Calibri is replaced by Liberation Sans here and the layout differs.

veraPDF locally: installer from <https://verapdf.org/software/> into `tools/verapdf` (found
automatically by Mimi) or any location referenced in PATH or via `VERAPDF=...`.

## Operation

```bash
lxc exec rampe -- systemctl status rampe
lxc exec rampe -- journalctl -u rampe -f            # log
lxc exec rampe -- apt-get upgrade                   # system packages; LibreOffice in /opt manually (see above)
```

- After a LibreOffice update run `cargo test --release -- --ignored`
  (regression test, see [FUNCTION.en.md](FUNCTION.en.md#testing)). Also export a real report
  and check it with veraPDF. The preparation (`prepare_for_pdf`) and the post-processing
  (`pdffix`) compensate for LibreOffice quirks.
- Mimi deletes reports (`/var/lib/rampe/data/jobs/`) automatically: 30 days after their last use
  (`JOB_TTL_DAYS` in `deploy/rampe.service`), a user's superseded reports after 1 day.
  Mimi cleans up at start and then hourly. The journal only shows the count, e.g.
  “3 Bericht(e) gelöscht (412 MB, Löschfrist abgelaufen)” (3 reports deleted, deadline expired).
- Backup: `/var/lib/rampe/users.txt` is sufficient, plus `audit.log` if needed. Reports can be uploaded again at any time.
  `users.txt` contains the two-factor secrets: protect backups just like the file itself.
- Reset two-factor for someone without a browser:
  `lxc exec rampe -- runuser -u rampe -- env USERS_FILE=/var/lib/rampe/users.txt DATA_DIR=/var/lib/rampe/data /opt/rampe/rampe zwei-faktor-aus NAME`
