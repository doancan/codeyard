#!/bin/sh
# First-time setup of the registry's host, which already runs Docker and Nginx Proxy Manager (and may run other things):
# run as root from this folder:
#   ./setup-host.sh
# yard comes in the published image (YARD_IMAGE in .env); a yard binary beside this script (the yard-deploy workflow's
# artifact, Linux x86_64) is installed into bin/ too, and wins over the image's.
# It creates the stack folder (/opt/yard) and the registry's folder (/srv/yard), the users `canary` and `deploy` with
# their sudo rules, and the root helpers. It changes nothing else on the machine: no firewall, no SSH, no updates, and
# no port is published. It can run again; it never touches an existing .env or /srv/yard's registry.
# docs/registry-setup-checklist.md has the steps before and after.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
docker_dir=$(cd "$here/.." && pwd)
stack=${YARD_STACK_DIR:-/opt/yard}
[ "$(id -u)" = 0 ] || { echo "Run it as root." >&2; exit 1; }
{ command -v docker >/dev/null && docker compose version >/dev/null; } || { echo "Docker with the compose plugin is needed first." >&2; exit 1; }

# The stack's files and the binary. The Dockerfile's folder is the build context, so ops/ goes along.
install -d -m 755 "$stack" "$stack/bin"
for f in Dockerfile .dockerignore compose.yml compose.npm.yml compose.port.yml litestream.yml; do
	install -m 644 "$docker_dir/$f" "$stack/$f"
done
rm -f "$stack/Caddyfile"
rm -rf "$stack/ops" "$stack/site"
cp -R "$docker_dir/ops" "$stack/ops"
cp -R "$docker_dir/site" "$stack/site"
chmod 755 "$stack"/ops/*
# A newer binary beside the script wins; without one, the installed binary stays (yard-install-binary replaces it).
if [ -s "$here/yard" ]; then
	install -m 755 "$here/yard" "$stack/bin/yard"
fi

# Secrets: one file, root only (a template on the first run).
if [ ! -f "$stack/.env" ]; then
	install -m 600 "$docker_dir/env.example" "$stack/.env"
fi

# The registry's folder belongs to the containers' user (10001), nobody on the host.
install -d -o 10001 -g 10001 -m 750 /srv/yard

install -d -m 755 /etc/yard /usr/local/libexec/yard
printf 'YARD_STACK_DIR=%s\n' "$stack" > /etc/yard/stack.conf
install -m 755 "$here/revoke" /usr/local/libexec/yard/revoke
install -m 755 "$here/install" /usr/local/libexec/yard/install
install -m 755 "$here/canary-revoke" /usr/local/bin/yard-canary-revoke
install -m 755 "$here/install-binary" /usr/local/bin/yard-install-binary
install -m 755 "$here/authorize" /usr/local/sbin/yard-authorize

# One user per workflow key, each allowed one command (yard-canary-revoke, yard-install-binary).
for u in canary deploy; do
	id "$u" >/dev/null 2>&1 || useradd --create-home --shell /bin/sh "$u"
done
cat > /etc/sudoers.d/yard <<'SUDO'
canary ALL=(root) NOPASSWD: /usr/local/libexec/yard/revoke 0.0.*
deploy ALL=(root) NOPASSWD: /usr/local/libexec/yard/install ""
SUDO
chmod 440 /etc/sudoers.d/yard
visudo -cf /etc/sudoers.d/yard

cat <<MSG

Done. Next (docs/registry-setup-checklist.md, "Go live"):
  1. Fill in $stack/.env (NPM_NETWORK, R2 and the alert channel), then:
       cd $stack && docker compose pull
       cd $stack && docker compose run --rm --no-deps ops sh -c 'echo test | yard-alert "a test alert"'
  2. cd $stack && docker compose run --rm --no-deps ops yard-init --registry market.yardcode.app --root-keys A,B,C --threshold 2
  3. Two root key holders sign /srv/yard/root.json; put root.json.sig back, then:  docker compose up -d
  4. In Nginx Proxy Manager: a proxy host for yardcode.app and market.yardcode.app → http://codeyard-yard:8080,
     with a Let's Encrypt certificate, Force SSL and HTTP/2.
  5. yard-authorize canary "ssh-ed25519 …"   and   yard-authorize deploy "ssh-ed25519 …"
MSG
