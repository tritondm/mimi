# Installation and operation

*Mimi – Macht Inhalte Mühelos Inklusiv*

[Deutsch](INSTALL.md) · **English**

How Mimi works internally: [FUNCTION.en.md](FUNCTION.en.md) · Usage and settings: [README.md](README.md)

> **Technical names:** program and command `mimi`, container `mimi`, service `mimi.service`,
> system user and paths `/opt/mimi`, `/var/lib/mimi`.

## Server overview

```
Internet ──https──▶ Host (LXD)
                     ├─ Container rproxy   nginx + Let's Encrypt, own address(es) → IP of container mimi, port 8080
                     └─ Container mimi    Ubuntu 26.04: Mimi (systemd), LibreOffice, veraPDF, fonts
```

| What | Where (in the container `mimi`) |
|---|---|
| Program | `/opt/mimi/mimi` (statically built, musl) |
| Service | `/etc/systemd/system/mimi.service` (template: `deploy/mimi.service`) |
| User | system user `mimi`, home `/var/lib/mimi` |
| User file | `/var/lib/mimi/users.txt` (mode 600, with the two-factor secrets) |
| Audit log | `/var/lib/mimi/data/audit.log` (mode 600, 90 days) |
| Reports | `/var/lib/mimi/data/jobs/` |
| LibreOffice profile | `/var/lib/mimi/data/lo-profile` (created by Mimi itself) |
| veraPDF | `/opt/verapdf/verapdf` |
| nginx configuration | container `rproxy`: `/etc/nginx/conf.d/mimi.conf` (template: `deploy/nginx-mimi.conf`) |

Versions in use (November 2026): Ubuntu 26.04.1 LTS, LibreOffice 26.8.3
(packages from The Document Foundation under `/opt/libreoffice26.8`), OpenJDK 25 (headless), veraPDF 1.30.

## Setting up a new server

All commands as `root` on the host.

### 1. Create the container

```bash
lxc launch ubuntu:26.04 mimi
lxc config set mimi limits.cpu 2
lxc config set mimi limits.memory 1536MiB
```

This is how the container is currently configured. LibreOffice only ever runs once at a
time; Mimi queues further exports.

### 2. Packages

```bash
lxc exec mimi -- apt-get update
lxc exec mimi -- apt-get install -y --no-install-recommends \
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
lxc exec mimi -- sh -c '
  cd /root && curl -fsSLO https://download.documentfoundation.org/libreoffice/stable/26.8.3/deb/x86_64/LibreOffice_26.8.3_Linux_x86-64_deb.tar.gz
  tar xzf LibreOffice_26.8.3_Linux_x86-64_deb.tar.gz && apt-get install -y ./LibreOffice_26.8.3*_deb/DEBS/*.deb
  apt-get install -y libxinerama1 libcups2t64 libsm6 libice6 libx11-xcb1 libxrandr2
  /opt/libreoffice26.8/program/soffice --version'
```

`deploy/mimi.service` sets `SOFFICE=/opt/libreoffice26.8/program/soffice`. For a newer
version adjust the path and version number.

### 3. User and directories

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

`deploy/verapdf-auto-install.xml` installs into `/opt/verapdf` without prompts.
The service finds it via `VERAPDF=/opt/verapdf/verapdf`.

### 5. Deploy the program

On the development machine (once: `rustup target add x86_64-unknown-linux-musl`):

```bash
deploy/deploy.sh root@host              # container mimi
deploy/deploy.sh root@host ct           # other container
```

You can also put the host into `deploy/local/deploy.env` once (`DEPLOY_HOST=root@host`);
then `deploy/deploy.sh` is enough. The folder `deploy/local/` does not belong in the repository.

The script builds the static program and copies the program and the service file into the
container. It then restarts the service and checks whether it answers via HTTP inside the
container.

### 6. Create the first admin

The password is prompted without echo (at least 12 characters):

```bash
lxc exec mimi -- runuser -u mimi -- env USERS_FILE=/var/lib/mimi/users.txt /opt/mimi/mimi benutzer-anlegen chef --admin
lxc exec mimi -- systemctl restart mimi
```

The admin creates further users in the browser under `/admin`.
Without an admin the service does not start; the journal then points to this command.

### 7. nginx (container `rproxy`)

```bash
lxc file push deploy/nginx-mimi.conf rproxy/etc/nginx/conf.d/mimi.conf
lxc exec rproxy -- certbot certonly --nginx --cert-name mimi.example.org -d mimi.example.org   # first time only
lxc exec rproxy -- sh -c 'nginx -t && systemctl reload nginx'
```

Important in the configuration: `client_max_body_size 300m` (large reports) and
`proxy_read_timeout 300s` (export and check of large reports take a while).
In the template, replace `mimi.example.org` with your own address and put the container's IP into
`upstream` (`lxc list mimi`). Several addresses work with one shared certificate (more `-d` for
certbot, all names in `server_name`). For a former address the template contains a block with a
permanent redirect (301). Logins are per address, so users log in once more after a switch.

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
needed. In the field-test report the entire body text ends up as Carlito in the PDF.

**Without an equivalent substitute:** Corbel, Tahoma, Consolas, Segoe UI, Aptos (new
default font from Word 2024). If they are used in body text, the layout differs.

**Check** which fonts are installed and which ones a PDF actually uses:

```bash
lxc exec mimi -- fc-list : family | sort -u
lxc exec mimi -- fc-match Calibri            # should return "Carlito"
pdffonts bericht.pdf                          # on your own machine (poppler-utils)
```

Mimi's basic check also lists the embedded fonts.

**Installing additional fonts** (e.g. the original fonts of the corporate template, provided
the licence allows use on a server; Microsoft fonts from Windows or Office are usually
**not** licensed for this):

```bash
lxc exec mimi -- install -d /usr/local/share/fonts/mimi
lxc file push Font-Regular.ttf Font-Bold.ttf mimi/usr/local/share/fonts/mimi/
lxc exec mimi -- fc-cache -f
lxc exec mimi -- systemctl restart mimi
```

## Development machine

```bash
cargo build --release
./target/release/mimi benutzer-anlegen chef --admin
./target/release/mimi                       # http://127.0.0.1:8080
```

Requires LibreOffice (`soffice` in PATH or `SOFFICE=...`). For the same results as on the
server install the same fonts, on Arch Linux e.g. `pacman -S ttf-carlito ttf-caladea`.
Without Carlito, Calibri is replaced by Liberation Sans here and the layout differs.

veraPDF locally: installer from <https://verapdf.org/software/> into `tools/verapdf` (found
automatically by Mimi) or any location referenced in PATH or via `VERAPDF=...`.

## Operation

```bash
lxc exec mimi -- systemctl status mimi
lxc exec mimi -- journalctl -u mimi -f            # log
lxc exec mimi -- apt-get upgrade                   # system packages; LibreOffice in /opt manually (see above)
```

- After a LibreOffice update run `cargo test --release -- --ignored`
  (regression test, see [FUNCTION.en.md](FUNCTION.en.md#testing)). Also export a real report
  and check it with veraPDF. The preparation (`prepare_for_pdf`) and the post-processing
  (`pdffix`) compensate for LibreOffice quirks.
- Mimi deletes reports (`/var/lib/mimi/data/jobs/`) automatically: 30 days after their last use
  (`JOB_TTL_DAYS` in `deploy/mimi.service`), a user's superseded reports after 1 day.
  Mimi cleans up at start and then hourly. The journal only shows the count, e.g.
  “3 Bericht(e) gelöscht (412 MB, Löschfrist abgelaufen)” (3 reports deleted, deadline expired).
- Backup: `/var/lib/mimi/users.txt` is sufficient, plus `audit.log` if needed. Reports can be uploaded again at any time.
  `users.txt` contains the two-factor secrets: protect backups just like the file itself.
- Reset two-factor for someone without a browser:
  `lxc exec mimi -- runuser -u mimi -- env USERS_FILE=/var/lib/mimi/users.txt DATA_DIR=/var/lib/mimi/data /opt/mimi/mimi zwei-faktor-aus NAME`
