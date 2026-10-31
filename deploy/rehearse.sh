#!/bin/sh
# The public registry, rehearsed end to end on this machine: no server, no accounts, no network (docs/registry.md,
# "Rehearsal"). It builds yard, cyx, the app and a stand-in Sigstore, then:
#   1. a root key ceremony with throwaway keys (scripts/yard-root-ceremony.sh --rehearsal)
#   2. yard init with those keys; root.json signed by one key is refused by clients, by two it is accepted
#   3. publishes a data-only extension, is refused code from a publisher who is not invited, publishes code from an
#      invited one
#   4. installs both with the app (`codeyard --install-extension`) from the local registry
#   5. revokes the code extension: the index says so, `codeyard --update-extensions` stops it on the client that has it,
#      and another client refuses to install it, with the reason
#   6. signs an index that has already expired: `yard status` (what the alert runs) fails, and clients refuse to
#      install from it and say why; a fresh signature ends that
#
#   deploy/rehearse.sh [WORK-FOLDER]
#
# REHEARSE_APP_FEATURES passes cargo features for the app build (on a Mac without Xcode's Metal toolchain:
# REHEARSE_APP_FEATURES=gpui/runtime_shaders). Exit status 0 only if every step behaved as described.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
work=${1:-$(mktemp -d "${TMPDIR:-/tmp}/yard-rehearsal.XXXXXX")}
mkdir -p "$work"
work=$(cd "$work" && pwd)
log="$work/log"
mkdir -p "$log"

# Each client keeps its settings and its extensions (store, registry caches, revoked.json) in the work folder, never
# in this user's own Codeyard folders.
unset CODEYARD_CONFIG_DIR CODEYARD_STATE_DIR
step() { printf '\n==> %s\n' "$*"; }
ok() { printf '    ok: %s\n' "$*"; }
die() {
	printf '\nREHEARSAL FAILED: %s\n' "$*" >&2
	for f in "$log"/*; do
		[ -s "$f" ] && { printf '\n--- %s\n' "$f" >&2; tail -n 30 "$f" >&2; }
	done
	exit 1
}
# expect_fail "what" "text the output must contain" command…
expect_fail() {
	what=$1 text=$2
	shift 2
	if out=$("$@" 2>&1); then
		die "$what: it succeeded: $out"
	fi
	printf '%s\n' "$out" | grep -qF -- "$text" || die "$what: the message does not say \"$text\": $out"
	ok "$what: $(printf '%s\n' "$out" | grep -F -- "$text" | head -n 1 | sed 's/^ *//')"
}

pids=""
cleanup() {
	for p in $pids; do kill "$p" 2>/dev/null || true; done
}
trap cleanup EXIT INT TERM

step "Build yard, cyx, the extension host, the app and the stand-in Sigstore"
cd "$here"
cargo build --locked -q -p yard -p cyx -p ext-host
cargo build --locked -q -p yard --example sigstore_stand_in
if [ -n "${REHEARSE_APP_FEATURES:-}" ]; then
	cargo build --locked -q -p app --bin codeyard --features "$REHEARSE_APP_FEATURES"
else
	cargo build --locked -q -p app --bin codeyard
fi
bin=$(cargo metadata --format-version 1 --no-deps | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug
yard=$bin/yard cyx=$bin/cyx app=$bin/codeyard stand_in=$bin/examples/sigstore_stand_in
for b in "$yard" "$cyx" "$app" "$stand_in" "$bin/codeyard-exthost"; do
	[ -x "$b" ] || die "missing $b"
done
ok "binaries in $bin"

port() { python3 -I -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
wait_for() {
	i=0
	until eval "$1" >/dev/null 2>&1; do
		i=$((i + 1))
		[ "$i" -lt 100 ] || die "timed out waiting for: $1"
		sleep 0.1
	done
}

step "Stand-in Sigstore (Fulcio and Rekor), so publishing signs without the network"
"$stand_in" serve "$work/sigstore" > "$log/sigstore" 2>&1 &
pids="$pids $!"
wait_for "test -s '$work/sigstore/url'"
sigstore=$(cat "$work/sigstore/url")
export CODEYARD_FULCIO_URL="$sigstore" CODEYARD_REKOR_URL="$sigstore"
ok "$sigstore"

step "Root key ceremony with throwaway keys"
CYX="$cyx" "$here/scripts/yard-root-ceremony.sh" --rehearsal "$work/ceremony" > "$log/ceremony"
key_a=$(sed -n 1p "$work/ceremony/root-keys.txt")
key_b=$(sed -n 2p "$work/ceremony/root-keys.txt")
key_c=$(sed -n 3p "$work/ceremony/root-keys.txt")
trust="--root-key $key_a --root-key $key_b --root-key $key_c --threshold 2"
ok "three keys, threshold 2; record in $work/ceremony/yard-ceremony-record.txt"

step "yard init with the root keys; the root waits for two signatures"
srv=$work/srv
"$yard" init "$srv" --registry rehearsal.yard.test --root-keys "$key_a,$key_b,$key_c" --threshold 2 \
	--fulcio "$work/sigstore/fulcio.pem" --rekor "$work/sigstore/rekor.pem" > "$log/init"
[ ! -e "$srv/keys/root.key" ] || die "a root key was written on the server"
yard_port=$(port)
url="http://127.0.0.1:$yard_port"
python3 -I - "$srv/yard.json" "127.0.0.1:$yard_port" <<'PY'
import json, sys
path, listen = sys.argv[1], sys.argv[2]
cfg = json.load(open(path))
cfg["listen"] = listen
cfg["invited"] = ["rehearse"]
json.dump(cfg, open(path, "w"), indent=2)
PY
"$cyx" yard sign root "$srv/root.json" --key "$work/ceremony/yard-root-a.key" > /dev/null
"$yard" serve "$srv/yard.json" > "$log/yard" 2>&1 &
pids="$pids $!"
wait_for "curl -fsS '$url/healthz'"
# shellcheck disable=SC2086
expect_fail "one root signature is not enough" "CY0804" "$cyx" yard check "$url" $trust
"$cyx" yard sign root "$srv/root.json" --key - < "$work/ceremony/yard-root-c.key" > /dev/null
"$yard" resign "$srv/yard.json" > /dev/null
# shellcheck disable=SC2086
"$cyx" yard check "$url" $trust > "$log/check" || die "the registry does not verify with two signatures: $(cat "$log/check")"
ok "two of three: $(cat "$log/check")"

step "Publish: data-only from anyone, code only from an invited publisher"
ext=$work/extensions
mkdir -p "$ext"
(cd "$ext" && "$cyx" new theme night --publisher hobby > /dev/null)
SIGSTORE_ID_TOKEN=$("$stand_in" token 1001 2001) "$cyx" publish "$ext/night" --registry "$url" > "$log/publish-night" ||
	die "publishing hobby.night: $(cat "$log/publish-night")"
ok "hobby.night 0.1.0 (data-only) published"
probe_manifest() {
	cat <<EOF
[extension]
id = "$1.probe"
version = "1.0.0"
name = "Probe"
license = "MIT"
description = "A code extension for the registry rehearsal."

[code]
entry = "Cargo.toml"

[permissions]
read = ["a.txt"]
net = ["api.example.com"]
auth = ["github"]
secrets = true
why = "Tests every part of the host."

[[commands]]
id = "notice"
title = "Probe: Notice"
EOF
}
for who in hobby rehearse; do
	mkdir -p "$ext/$who-probe"
	probe_manifest "$who" > "$ext/$who-probe/codeyard-extension.toml"
	: > "$ext/$who-probe/Cargo.toml"
	cp "$here/crates/ext-host/testdata/probe.wasm" "$ext/$who-probe/extension.wasm"
done
expect_fail "code from a publisher who is not invited" "accepts data-only extensions" \
	env SIGSTORE_ID_TOKEN="$("$stand_in" token 1001 2001)" "$cyx" publish "$ext/hobby-probe" --registry "$url"
SIGSTORE_ID_TOKEN=$("$stand_in" token 1002 2002) "$cyx" publish "$ext/rehearse-probe" --registry "$url" > "$log/publish-probe" ||
	die "publishing rehearse.probe: $(cat "$log/publish-probe")"
ok "rehearse.probe 1.0.0 (code, invited) published"

step "Install both with the app from the local registry"
client() {
	mkdir -p "$1"
	cat > "$1/settings.jsonc" <<EOF
{ "extensions.registries": [ { "name": "rehearsal", "url": "$url", "root-keys": ["$key_a", "$key_b", "$key_c"], "threshold": 2 } ] }
EOF
}
client "$work/client-1"
CODEYARD_CONFIG_DIR="$work/client-1" CODEYARD_STATE_DIR="$work/client-1/state" "$app" --install-extension hobby.night --install-extension rehearse.probe > "$log/install" 2>&1 ||
	die "installing: $(cat "$log/install")"
for line in "installed hobby.night 0.1.0" "installed rehearse.probe 1.0.0"; do
	grep -qF "$line" "$log/install" || die "installing: $(cat "$log/install")"
done
ok "$(tr '\n' ';' < "$log/install" | sed 's/;$//; s/;/; /g')"

step "Revoke rehearse.probe 1.0.0"
"$yard" revoke "$srv/yard.json" rehearse.probe 1.0.0 "Rehearsal: a revoked version stops on every client." > /dev/null
# shellcheck disable=SC2086
"$cyx" yard check "$url" $trust --revoked rehearse.probe@1.0.0 > "$log/check" || die "the index does not show the revocation: $(cat "$log/check")"
ok "in the signed index: $(cat "$log/check")"
CODEYARD_CONFIG_DIR="$work/client-1" CODEYARD_STATE_DIR="$work/client-1/state" "$app" --update-extensions > "$log/update" 2>&1 || die "--update-extensions: $(cat "$log/update")"
grep -q "Rehearsal: a revoked version stops on every client." "$log/update" || die "the client did not stop it: $(cat "$log/update")"
grep -q rehearse.probe "$work/client-1/state/extensions/revoked.json" || die "no revoked.json on the client"
ok "the client that has it stops it: $(head -n 1 "$log/update")"
client "$work/client-2"
expect_fail "another client refuses to install it" "was revoked by the registry" \
	env CODEYARD_CONFIG_DIR="$work/client-2" CODEYARD_STATE_DIR="$work/client-2/state" "$app" --install-extension rehearse.probe@1.0.0
curl -fsS "$url/metrics" | grep -q '^yard_versions_revoked 1$' || die "/metrics does not count the revocation"
ok "/metrics: yard_versions_revoked 1"

step "An expired index blocks installs, and says why"
"$yard" resign "$srv/yard.json" --expired > /dev/null
expect_fail "yard status (the hourly alert check)" "the index has expired" "$yard" status "$srv/yard.json"
# shellcheck disable=SC2086
expect_fail "cyx yard check (the outside monitor)" "the index has expired" "$cyx" yard check "$url" $trust
client "$work/client-3"
expect_fail "a client installing from it" "more than 3 days old" \
	env CODEYARD_CONFIG_DIR="$work/client-3" CODEYARD_STATE_DIR="$work/client-3/state" "$app" --install-extension hobby.night
"$yard" resign "$srv/yard.json" > /dev/null
"$yard" status "$srv/yard.json" > "$log/status" || die "after a fresh signature: $(cat "$log/status")"
ok "after the next daily signature: $(cat "$log/status")"

printf '\nRehearsal passed (work folder %s).\n' "$work"
