#!/bin/sh
# First-time setup of the registry's server: a fresh Ubuntu 24.04 LTS (Hetzner Cloud CX22 or larger) behind
# Cloudflare. Run as root from this folder, with these beside it:
#   yard         the yard binary (Linux x86_64, the yard-deploy workflow's artifact, a release build)
#   origin.pem   the Cloudflare Origin CA certificate for the address (SSL/TLS → Origin Server → Create certificate)
#   origin.key   its private key
# then:
#   ./setup.sh yard.example.dev
# It can run again: it changes nothing that is already in place, and never touches /srv/yard's registry or
# /etc/yard/yard.env once they exist. docs/registry-setup-checklist.md has the steps before and after.
set -eu
domain=${1:?usage: setup.sh DOMAIN}
here=$(cd "$(dirname "$0")" && pwd)
[ "$(id -u)" = 0 ] || { echo "Run it as root." >&2; exit 1; }
for f in yard origin.pem origin.key; do
	[ -s "$here/$f" ] || { echo "Put $f beside this script first (see its first lines)." >&2; exit 1; }
done
chmod 755 "$here/yard"   # a downloaded workflow artifact loses its executable bit
"$here/yard" --version >/dev/null || { echo "$here/yard does not run here (it is the Linux x86_64 build)." >&2; exit 1; }

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y caddy rclone ufw unattended-upgrades sqlite3 curl python3

# Litestream copies the database to R2 as it changes. Pinned by version and checksum.
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

# SSH by key only.
cat > /etc/ssh/sshd_config.d/10-yard.conf <<'EOF'
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
EOF
systemctl reload ssh

# Logs rotate by size and age.
mkdir -p /etc/systemd/journald.conf.d
install -m 644 "$here/journald.conf" /etc/systemd/journald.conf.d/yard.conf
systemctl restart systemd-journald

# The service runs as its own user with only /srv/yard writable (yard.service).
id yard >/dev/null 2>&1 || useradd --system --home-dir /srv/yard --shell /usr/sbin/nologin yard
install -d -o yard -g yard -m 750 /srv/yard
install -m 755 "$here/yard" /usr/local/bin/yard
install -m 755 "$here/canary-revoke" /usr/local/bin/yard-canary-revoke
install -m 755 "$here/install-binary" /usr/local/bin/yard-install-binary
install -m 755 "$here/alert" /usr/local/bin/yard-alert
install -m 755 "$here/watch" /usr/local/bin/yard-watch
install -m 755 "$here/backup" /usr/local/bin/yard-backup
install -m 755 "$here/restore" /usr/local/bin/yard-restore
install -m 755 "$here/authorize" /usr/local/sbin/yard-authorize
install -m 755 "$here/firewall" /usr/local/sbin/yard-firewall
for u in yard.service yard-alert@.service yard-resign.service yard-resign.timer yard-watch.service yard-watch.timer \
	yard-backup.service yard-backup.timer yard-restore-check.service yard-restore-check.timer \
	yard-firewall.service yard-firewall.timer; do
	install -m 644 "$here/$u" /etc/systemd/system/
done

# Secrets: one file, root only (a template on the first run).
install -d -m 700 /etc/yard
if [ ! -f /etc/yard/yard.env ]; then
	sed "s/^YARD_DOMAIN=.*/YARD_DOMAIN=$domain/" "$here/yard.env.example" > /etc/yard/yard.env
	chmod 600 /etc/yard/yard.env
fi

# TLS: Cloudflare's origin certificate, readable by Caddy only.
install -m 644 -o root -g caddy "$here/origin.pem" /etc/caddy/origin.pem
install -m 640 -o root -g caddy "$here/origin.key" /etc/caddy/origin.key
install -m 644 "$here/Caddyfile" /etc/caddy/Caddyfile
printf 'YARD_DOMAIN=%s\n' "$domain" > /etc/caddy/yard.env
mkdir -p /etc/systemd/system/caddy.service.d
printf '[Unit]\nOnFailure=yard-alert@%%n.service\n[Service]\nEnvironmentFile=/etc/caddy/yard.env\n' > /etc/systemd/system/caddy.service.d/yard.conf
YARD_DOMAIN=$domain caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile

# Backups: Litestream reads the bucket and keys from yard.env.
install -m 644 "$here/litestream.yml" /etc/litestream.yml
mkdir -p /etc/systemd/system/litestream.service.d
printf '[Unit]\nOnFailure=yard-alert@%%n.service\n[Service]\nEnvironmentFile=/etc/yard/yard.env\n' > /etc/systemd/system/litestream.service.d/yard.conf

# Firewall: SSH, and HTTPS from Cloudflare only.
/usr/local/sbin/yard-firewall

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
systemctl enable yard litestream yard-resign.timer yard-watch.timer yard-backup.timer yard-restore-check.timer yard-firewall.timer
systemctl start yard-firewall.timer

cat <<EOF

Done. Next (docs/registry-setup-checklist.md, "Go live"):
  1. Fill in /etc/yard/yard.env (R2 and the alert channel), then:  echo test | yard-alert "a test alert"
  2. sudo -u yard yard init /srv/yard --registry $domain --root-keys A,B,C --threshold 2
  3. Two root key holders sign /srv/yard/root.json; put root.json.sig back, then:
     systemctl start yard litestream yard-resign.timer yard-watch.timer yard-backup.timer yard-restore-check.timer
  4. yard-authorize canary "ssh-ed25519 …"   and   yard-authorize deploy "ssh-ed25519 …"
EOF
