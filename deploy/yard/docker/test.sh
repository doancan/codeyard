#!/bin/sh
# The Docker stack, tested end to end on a developer machine: no server, no Cloudflare, no R2 (docs/registry.md,
# "Running on Docker"). It brings up the stack with throwaway root keys and MinIO as the stand-in R2, then checks:
#   1. the stack publishes no host port, and comes up healthy
#   2. `cyx yard check` passes, a proxied /metrics answers 404, and the site answers
#   3. yard-backup writes to the bucket (files, a database snapshot, Litestream's copy) and yard-restore --check reads it back
#   4. stopping yard makes yard-watch fail, and the alert reaches the (stand-in) channel
#   5. the canary and deploy forced commands take their one command and nothing else; the root helpers do their work:
#      a revoke reaches yard, a good binary is installed, a binary that does not run is refused, one that runs but never
#      gets healthy is rolled back
#
#   deploy/yard/docker/test.sh
#
# YARD_LINUX_BIN names a Linux yard binary for this machine's architecture; without it, yard is built in a Rust
# container (a few minutes the first time). CYX names the cyx binary (default: target/release/cyx, built if missing).
# Needs Docker with the compose plugin. Exit status 0 only if every step behaved as described.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
work=$(mktemp -d /tmp/yard-docker-test.XXXXXX)
stack=$work/stack
project=yardtest

step() { printf '\n==> %s\n' "$*"; }
ok() { printf '    ok: %s\n' "$*"; }
die() {
	printf '\nDOCKER TEST FAILED: %s\n' "$*" >&2
	(cd "$stack" 2>/dev/null && docker compose logs --tail 30 2>&1 | tail -n 80 >&2) || true
	exit 1
}
cleanup() {
	(cd "$stack" 2>/dev/null && docker compose down -v --remove-orphans >/dev/null 2>&1) || true
	rm -rf "$work"
}
trap cleanup EXIT

command -v docker >/dev/null && docker compose version >/dev/null 2>&1 || die "Docker with the compose plugin is needed"

step "The yard binary for Linux and the cyx client"
mkdir -p "$stack/bin"
if [ -n "${YARD_LINUX_BIN:-}" ]; then
	cp "$YARD_LINUX_BIN" "$stack/bin/yard"
else
	docker run --rm -v "$repo":/src:ro -v yard-cargo-target:/target -v yard-cargo-home:/usr/local/cargo/registry \
		-e CARGO_TARGET_DIR=/target -v "$stack/bin":/out -w /src rust:1-bookworm \
		sh -c 'cargo build --release --locked -p yard && cp /target/release/yard /out/yard' > "$work/build.log" 2>&1 ||
		{ tail -n 30 "$work/build.log" >&2; die "building yard in a container"; }
fi
chmod 755 "$stack/bin/yard"
cyx=${CYX:-$repo/target/release/cyx}
if [ ! -x "$cyx" ]; then
	(cd "$repo" && cargo build --release --locked -q -p cyx) || die "building cyx"
fi
ok "yard $(docker run --rm --entrypoint /staged/yard -v "$stack/bin/yard":/staged/yard:ro ubuntu:24.04 --version 2>&1)"

step "The stack folder, as the server has it, with the test's own .env"
cp "$here/Dockerfile" "$here/.dockerignore" "$here/compose.yml" "$here/compose.test.yml" "$here/litestream.yml" "$stack/"
cp -R "$here/ops" "$stack/ops"
cp -R "$here/site" "$stack/site"
port=$(python3 -I -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
cat > "$stack/.env" <<EOF
YARD_DOMAIN=test.yard.test
YARD_TEST_PORT=$port
YARD_R2_BUCKET=yard-test
YARD_R2_ENDPOINT=http://minio:9000
YARD_R2_ACCESS_KEY_ID=yardtest
YARD_R2_SECRET_ACCESS_KEY=yardtest-secret
YARD_ALERT_WEBHOOK=http://hook:8080/alert
EOF
chmod 600 "$stack/.env"
export COMPOSE_PROJECT_NAME=$project COMPOSE_FILE="$stack/compose.yml:$stack/compose.test.yml"
cd "$stack"
dc() { docker compose "$@"; }
ops() { dc exec -T ops "$@"; }
url=http://127.0.0.1:$port

step "No host port in the stack itself"
published=$(docker compose -f "$stack/compose.yml" config --format json | python3 -I -c 'import json,sys; c=json.load(sys.stdin); print(sum(len(s.get("ports") or []) for s in c["services"].values()))')
[ "$published" = 0 ] || die "compose.yml publishes $published host port(s)"
ok "compose.yml publishes none (the test adds one for yard, in compose.test.yml)"

step "Build, and a registry with three throwaway root keys (two of three sign)"
dc build >"$work/compose-build.log" 2>&1 || { tail -n 30 "$work/compose-build.log" >&2; die "docker compose build"; }
CYX="$cyx" "$repo/scripts/yard-root-ceremony.sh" --rehearsal "$work/ceremony" > "$work/ceremony.log"
key_a=$(sed -n 1p "$work/ceremony/root-keys.txt")
key_b=$(sed -n 2p "$work/ceremony/root-keys.txt")
key_c=$(sed -n 3p "$work/ceremony/root-keys.txt")
trust="--root-key $key_a --root-key $key_b --root-key $key_c --threshold 2"
dc up -d minio createbucket hook >/dev/null 2>&1 || die "minio did not start"
dc run --rm --no-deps -T ops yard-init --registry test.yard.test --root-keys "$key_a,$key_b,$key_c" --threshold 2 > "$work/init.log" 2>&1 ||
	{ cat "$work/init.log" >&2; die "yard-init"; }
dc run --rm --no-deps -T ops cat /srv/yard/root.json > "$work/root.json"
"$cyx" yard sign root "$work/root.json" --key "$work/ceremony/yard-root-a.key" > /dev/null
"$cyx" yard sign root "$work/root.json" --key - < "$work/ceremony/yard-root-c.key" > /dev/null
dc run --rm --no-deps -T ops sh -c 'cat > /srv/yard/root.json.sig' < "$work/root.json.sig"
listen=$(dc run --rm --no-deps -T ops jq -r .listen /srv/yard/yard.json)
[ "$listen" = "0.0.0.0:8080" ] || die "yard-init left listen at $listen"
ok "registry created, listening on the compose network, root signed by A and C"

step "docker compose up: every service healthy"
dc up -d --wait --wait-timeout 180 >"$work/up.log" 2>&1 || { cat "$work/up.log" >&2; die "the stack did not become healthy"; }
for s in yard litestream ops; do
	id=$(dc ps -q "$s")
	[ "$(docker inspect -f '{{.State.Health.Status}}' "$id")" = healthy ] || die "$s is not healthy"
done
ok "yard, litestream and ops are healthy"

step "cyx yard check; /metrics is refused to a proxy; the site answers"
# shellcheck disable=SC2086
"$cyx" yard check "$url" $trust > "$work/check.log" 2>&1 || { cat "$work/check.log" >&2; die "cyx yard check"; }
ok "$(cat "$work/check.log")"
code=$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 192.0.2.1' "$url/metrics")
[ "$code" = 404 ] || die "a proxied /metrics answered $code, not 404"
[ "$(curl -s -o /dev/null -w '%{http_code}' "$url/healthz")" = 200 ] || die "/healthz does not answer 200"
ok "proxied /metrics 404, /healthz 200"
curl -fsS "$url/" | grep -q '<title>Codeyard' || die "the site does not answer at /"
curl -fsS "$url/extensions/codeyard.canary" | grep -q '<title>Codeyard' || die "an extension page does not answer"
curl -fsS "$url/app.js" | grep -q 'loadIndex' || die "the site's script does not answer"
[ "$(curl -s -o /dev/null -w '%{http_code}' "$url/missing.css")" = 404 ] || die "a missing asset is not 404"
ok "the site answers /, an extension page and its script"
big=$(head -c 136000000 /dev/zero | curl -s --max-time 120 -o /dev/null -w '%{http_code}' -X POST --data-binary @- "$url/v1/publish" || true)
[ "$big" = 413 ] || die "an upload over the body limit answered $big, not 413"
ok "an upload over the body limit answers 413"

step "yard-watch passes; yard-backup, then yard-restore --check"
ops yard-watch > "$work/watch.log" 2>&1 || { cat "$work/watch.log" >&2; die "yard-watch on a healthy stack"; }
ok "$(grep -m1 . "$work/watch.log")"
ops yard-backup > "$work/backup.log" 2>&1 || { cat "$work/backup.log" >&2; die "yard-backup"; }
ok "$(cat "$work/backup.log")"
i=0
until ops sh -c 'RCLONE_S3_PROVIDER=Cloudflare RCLONE_S3_ENDPOINT=$YARD_R2_ENDPOINT RCLONE_S3_REGION=auto RCLONE_S3_ACCESS_KEY_ID=$YARD_R2_ACCESS_KEY_ID RCLONE_S3_SECRET_ACCESS_KEY=$YARD_R2_SECRET_ACCESS_KEY RCLONE_S3_NO_CHECK_BUCKET=true rclone lsf -R :s3:$YARD_R2_BUCKET' > "$work/bucket.txt" 2>&1 &&
	grep -q '^litestream/' "$work/bucket.txt"; do
	i=$((i + 1))
	[ "$i" -lt 30 ] || { cat "$work/bucket.txt" >&2; die "Litestream's copy is not in the bucket"; }
	sleep 2
done
grep -q '^files/root.json$' "$work/bucket.txt" || die "files/root.json is not in the bucket"
grep -q '^snapshots/yard-.*\.sqlite\.gz$' "$work/bucket.txt" || die "no database snapshot in the bucket"
grep -q 'online.key' "$work/bucket.txt" && die "the online key is in the bucket"
ok "the bucket has files/, snapshots/ and litestream/, and no online key"
ops yard-restore --check > "$work/restore.log" 2>&1 || { cat "$work/restore.log" >&2; die "yard-restore --check"; }
ok "$(cat "$work/restore.log")"
if ops yard-restore > "$work/restore-live.log" 2>&1; then
	die "yard-restore ran onto a folder that has a registry"
fi
grep -q 'already has a registry' "$work/restore-live.log" || die "yard-restore's refusal does not say why: $(cat "$work/restore-live.log")"
ok "yard-restore (not --check) refuses the live folder"

step "Killing yard makes yard-watch fail, and the alert is sent"
dc stop yard >/dev/null 2>&1
if ops yard-job watch yard-watch > "$work/watch-down.log" 2>&1; then
	die "yard-watch passed with yard stopped"
fi
grep -q 'yard does not answer' "$work/watch-down.log" || die "yard-watch does not name the problem: $(cat "$work/watch-down.log")"
posts=$(dc exec -T hook cat /hook/posts.log 2>/dev/null || true)
printf '%s' "$posts" | grep -q 'watch failed on test.yard.test' || die "no alert arrived at the channel: $posts"
ok "yard-watch failed ($(grep -m1 'yard-watch:' "$work/watch-down.log")), the alert arrived"
dc up -d --wait --wait-timeout 120 yard >/dev/null 2>&1 || die "yard did not come back"
ops yard-watch >/dev/null 2>&1 || die "yard-watch fails after yard is back"
ok "yard is back, yard-watch passes"

step "Forced commands take their one command and nothing else"
mkdir -p "$work/fakebin"
cat > "$work/fakebin/sudo" <<'SH'
#!/bin/sh
# stands in for sudo: records what would run as root
printf '%s\n' "$@" > "$SUDO_LOG"
SH
chmod 755 "$work/fakebin/sudo"
SUDO_LOG=$work/sudo.log
export SUDO_LOG
forced() { # forced SCRIPT "ssh original command"
	script=$1 cmd=$2
	rm -f "$SUDO_LOG"
	PATH="$work/fakebin:$PATH" SSH_ORIGINAL_COMMAND="$cmd" sh "$here/host/$script" > "$work/forced.out" 2>&1
}
forced canary-revoke "revoke 0.0.7" || die "canary-revoke refused its own command"
[ "$(tr '\n' ' ' < "$SUDO_LOG")" = "-n /usr/local/libexec/yard/revoke 0.0.7 " ] || die "canary-revoke asked sudo for: $(cat "$SUDO_LOG")"
ok "canary: revoke 0.0.7 reaches the helper, and only that"
for bad in "" "id" "revoke" "revoke 1.0.0" "revoke 0.0.7 extra" "revoke 0.0.7; id" 'revoke $(id)' "revoke 0.0.1234567890" "revoke -0.0.1" "ls /"; do
	if forced canary-revoke "$bad"; then die "canary-revoke took: $bad"; fi
	[ ! -e "$SUDO_LOG" ] || die "canary-revoke called sudo for: $bad"
done
ok "canary: ten other commands are refused before sudo"
forced install-binary "" < /dev/null || die "install-binary refused standard input with no command"
[ "$(tr '\n' ' ' < "$SUDO_LOG")" = "-n /usr/local/libexec/yard/install " ] || die "install-binary asked sudo for: $(cat "$SUDO_LOG")"
for bad in "id" "cat /etc/shadow" "scp -t /tmp" "yard-install-binary; id"; do
	if forced install-binary "$bad" < /dev/null; then die "install-binary took: $bad"; fi
	[ ! -e "$SUDO_LOG" ] || die "install-binary called sudo for: $bad"
done
ok "deploy: no command at all, only the binary on standard input"
for bad in "0.0.7 extra" "1.0.0" "" "0.0.7;id"; do
	# shellcheck disable=SC2086
	if YARD_STACK_DIR="$stack" sh "$here/host/revoke" $bad > "$work/revoke.out" 2>&1; then die "the revoke helper took: $bad"; fi
	grep -q 'only: revoke 0.0.N' "$work/revoke.out" || die "the revoke helper's refusal of '$bad' says: $(cat "$work/revoke.out")"
done
ok "the root revoke helper checks its argument again"

step "The root helpers: revoke reaches yard; install swaps, refuses, and rolls back"
if YARD_STACK_DIR="$stack" sh "$here/host/revoke" 0.0.7 > "$work/revoke.out" 2>&1; then
	die "revoking a version nobody published succeeded: $(cat "$work/revoke.out")"
fi
grep -qi 'codeyard.canary' "$work/revoke.out" || die "the revoke did not reach yard (its answer should name codeyard.canary): $(cat "$work/revoke.out")"
ok "revoke 0.0.7 reached yard: $(head -n 1 "$work/revoke.out")"
installer() { YARD_STACK_DIR="$stack" YARD_HEALTH_WAIT=${YARD_HEALTH_WAIT:-40} sh "$here/host/install"; }
cp "$stack/bin/yard" "$work/yard.good"
installer < "$work/yard.good" > "$work/install.out" 2>&1 || { cat "$work/install.out" >&2; die "installing a good binary"; }
[ -x "$stack/bin/yard.previous" ] || die "the previous binary was not kept"
ok "good binary: $(tail -n 1 "$work/install.out"); yard.previous kept"
printf '#!/bin/sh\necho "not a yard"\nexit 1\n' > "$work/yard.broken"
if installer < "$work/yard.broken" > "$work/install-broken.out" 2>&1; then die "a binary that does not run was installed"; fi
cmp -s "$stack/bin/yard" "$work/yard.good" || die "a binary that does not run replaced the good one"
ok "a binary that does not run is refused and bin/yard is untouched"
printf '#!/bin/sh\n[ "$1" = "--version" ] && { echo "yard 9.9.9"; exit 0; }\nexit 1\n' > "$work/yard.unhealthy"
if installer < "$work/yard.unhealthy" > "$work/install-unhealthy.out" 2>&1; then die "a binary that never became healthy was kept"; fi
grep -q 'previous binary is back and healthy' "$work/install-unhealthy.out" || die "no rollback: $(tail -n 5 "$work/install-unhealthy.out")"
cmp -s "$stack/bin/yard" "$work/yard.good" || die "the rollback did not restore the good binary"
dc exec -T yard yard --version > /dev/null || die "yard is not running after the rollback"
ok "a binary that runs but never gets healthy is rolled back, and yard is healthy again"

step "Still serving, and the registry verifies"
# shellcheck disable=SC2086
"$cyx" yard check "$url" $trust > /dev/null 2>&1 || die "cyx yard check fails after the deploy tests"
ok "cyx yard check passes"

printf '\nDOCKER TEST PASSED\n'
