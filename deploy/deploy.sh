#!/usr/bin/env bash
# Baut das statische Binary und spielt es in den LXD-Container "rampe" auf dem Host.
# Nutzung: deploy/deploy.sh [root@host] [container]
# Einmalige Einrichtung (Container, LibreOffice, Schriften, veraPDF, nginx): siehe INSTALL.md
set -euo pipefail
cd "$(dirname "$0")/.."
HOST="${1:-root@server.example.org}"
CT="${2:-rampe}"
APP=rampe
BIN=target/x86_64-unknown-linux-musl/release/$APP

export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --target x86_64-unknown-linux-musl

scp -q "$BIN" "$HOST":/tmp/$APP.new
scp -q deploy/$APP.service "$HOST":/tmp/$APP.service
ssh "$HOST" "
  set -e
  lxc exec $CT -- mkdir -p /opt/$APP
  lxc file push -q /tmp/$APP.new $CT/opt/$APP/$APP.new --mode 0755
  lxc file push -q /tmp/$APP.service $CT/etc/systemd/system/$APP.service
  rm -f /tmp/$APP.new /tmp/$APP.service
  lxc exec $CT -- sh -c '
    mv /opt/$APP/$APP.new /opt/$APP/$APP
    systemctl daemon-reload
    systemctl enable $APP >/dev/null 2>&1
    systemctl restart $APP
    sleep 1
    systemctl is-active $APP
    curl -fsS -o /dev/null -w \"im Container: HTTP %{http_code}\n\" http://127.0.0.1:8080/
  '
"
