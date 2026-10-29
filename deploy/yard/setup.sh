#!/bin/sh
# First-time setup of the registry's server, a fresh Ubuntu 24.04 LTS (Hetzner CX22). Run as root from this folder,
# with the yard binary (Linux x86_64, from the deploy workflow's artifact) beside it:
#   ./setup.sh yard.example.dev you@example.dev
# It can run again: it changes nothing that is already in place. docs/registry.md, "Running the public registry",
# has the steps before and after.
set -eu
domain=${1:?usage: setup.sh DOMAIN EMAIL}
email=${2:?usage: setup.sh DOMAIN EMAIL}
here=$(cd "$(dirname "$0")" && pwd)
[ -x "$here/yard" ] || { echo "Put the yard binary (Linux x86_64) beside this script first." >&2; exit 1; }

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y caddy rclone ufw unattended-upgrades sqlite3 curl

# Litestream copies the database to the backup bucket as it changes. Pinned by version and checksum.
LITESTREAM_VERSION=0.5.17
LITESTREAM_SHA256=a191a0928884d1820fab1f866ede1d0d5811c323d0587bf863a43481a82a7668
if ! command -v litestream >/dev/null || ! litestream version | grep -q "$LITESTREAM_VERSION"; then
	curl -fsSL -o /tmp/litestream.deb \
		"https://github.com/benbjohnson/litestream/releases/download/v$LITESTREAM_VERSION/litestream-$LITESTREAM_VERSION-linux-x86_64.deb"
	echo "$LITESTREAM_SHA256  /tmp/litestream.deb" | sha256sum -c -
	apt-get install -y /tmp/litestream.deb
	rm -f /tmp/litestream.deb
fi

# Security updates install by themselves; when one needs a reboot, it happens at 04:00 (yard starts again with it).
cat > /etc/apt/apt.conf.d/20auto-upgrades <<'EOF'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
EOF
cat > /etc/apt/apt.conf.d/52codeyard-reboot <<'EOF'
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-Time "04:00";
EOF

# Only SSH and the web are reachable.
ufw allow OpenSSH
ufw allow 80/tcp
ufw allow 443/tcp
ufw --force enable

# The service runs as its own user with only /srv/yard writable (yard.service).
id yard >/dev/null 2>&1 || useradd --system --home-dir /srv/yard --shell /usr/sbin/nologin yard
install -d -o yard -g yard -m 750 /srv/yard
install -m 755 "$here/yard" /usr/local/bin/yard
install -m 755 "$here/canary-revoke" /usr/local/bin/yard-canary-revoke
install -m 755 "$here/install-binary" /usr/local/bin/yard-install-binary
install -m 755 "$here/authorize" /usr/local/sbin/yard-authorize
install -m 644 "$here/yard.service" "$here/yard-backup.service" "$here/yard-backup.timer" /etc/systemd/system/

# TLS and the address.
install -m 644 "$here/Caddyfile" /etc/caddy/Caddyfile
printf 'YARD_DOMAIN=%s\nYARD_EMAIL=%s\n' "$domain" "$email" > /etc/caddy/yard.env
mkdir -p /etc/systemd/system/caddy.service.d
printf '[Service]\nEnvironmentFile=/etc/caddy/yard.env\n' > /etc/systemd/system/caddy.service.d/yard.conf

# Backups: the bucket and its keys go in /etc/yard/backup.env (a template on the first run).
install -d -m 700 /etc/yard
[ -f /etc/yard/backup.env ] || install -m 600 "$here/backup.env.example" /etc/yard/backup.env
install -m 644 "$here/litestream.yml" /etc/litestream.yml
mkdir -p /etc/systemd/system/litestream.service.d
printf '[Service]\nEnvironmentFile=/etc/yard/backup.env\n' > /etc/systemd/system/litestream.service.d/yard.conf

# One user per workflow key, each allowed one command (canary-revoke, install-binary).
for u in canary deploy; do
	id "$u" >/dev/null 2>&1 || useradd --create-home --shell /bin/sh "$u"
done
cat > /etc/sudoers.d/yard <<'EOF'
canary ALL=(yard) NOPASSWD: /usr/local/bin/yard revoke /srv/yard/yard.json codeyard.canary *
deploy ALL=(root) NOPASSWD: /usr/bin/install -m 755 /usr/local/bin/yard /usr/local/bin/yard.previous, /usr/bin/install -m 755 /tmp/yard.* /usr/local/bin/yard, /usr/bin/systemctl restart yard, /usr/bin/systemctl is-active yard
EOF
chmod 440 /etc/sudoers.d/yard
visudo -cf /etc/sudoers.d/yard

systemctl daemon-reload
systemctl enable --now caddy
systemctl restart caddy
systemctl enable yard yard-backup.timer litestream

cat <<EOF

Done. Next (docs/registry.md, "Running the public registry"):
  1. Fill in /etc/yard/backup.env, then: systemctl restart litestream && systemctl start yard-backup.timer
  2. sudo -u yard yard init /srv/yard --registry $domain --root-keys A,B,C --threshold 2
  3. Have the root key holders sign /srv/yard/root.json, put root.json.sig back, then: systemctl start yard
  4. yard-authorize canary "ssh-ed25519 …"   and   yard-authorize deploy "ssh-ed25519 …"
EOF
