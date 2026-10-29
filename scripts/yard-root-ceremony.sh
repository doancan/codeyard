#!/bin/sh
# The extension registry's root key ceremony (docs/registry.md, "Root key ceremony"). Run it once, on your own
# computer, ideally with the network off:
#   scripts/yard-root-ceremony.sh /Volumes/YARD-A /Volumes/YARD-B
# It makes three root keys, any two of which sign the registry's root.json:
#   C  shown once, for a secure note in your password manager (Bitwarden); never left on disk
#   A  on the first encrypted USB stick
#   B  on the second encrypted USB stick, kept somewhere else
# and writes yard-ceremony-record.txt here: the date, the public keys and where each private key is kept (no secret).
# CYX names the cyx binary when it is not on PATH.
set -eu
a=${1:?usage: yard-root-ceremony.sh USB-A USB-B}
b=${2:?usage: yard-root-ceremony.sh USB-A USB-B}
cyx=${CYX:-cyx}
command -v "$cyx" >/dev/null || { echo "cyx not found: install it, or set CYX=/path/to/cyx" >&2; exit 1; }
for d in "$a" "$b"; do
	[ -d "$d" ] || { echo "$d is not a folder: insert and mount the USB stick" >&2; exit 1; }
done
[ "$(cd "$a" && pwd -P)" != "$(cd "$b" && pwd -P)" ] || { echo "A and B must be two different sticks" >&2; exit 1; }
key_a="$a/yard-root-a.key"
key_b="$b/yard-root-b.key"
for f in "$key_a" "$key_b"; do
	[ ! -e "$f" ] || { echo "$f exists: this ceremony was run before; keep those keys" >&2; exit 1; }
done
if command -v diskutil >/dev/null; then
	for d in "$a" "$b"; do
		if ! diskutil info "$d" 2>/dev/null | grep -Eq 'FileVault: +Yes'; then
			echo "warning: $d does not look encrypted. Erase it in Disk Utility as \"APFS (Encrypted)\" first." >&2
			printf 'Continue anyway? Type yes: '
			read -r ans
			[ "$ans" = yes ] || exit 1
		fi
	done
fi
public() { sed -n 's/^public key //p'; }

tmp=$(mktemp -d "${TMPDIR:-/tmp}/yard-ceremony.XXXXXX")
trap 'rm -rf "$tmp"' EXIT INT TERM
pub_c=$("$cyx" yard keygen "$tmp/c.key" | public)
cat <<EOF

Key C. Create a Secure Note in Bitwarden named "Codeyard Yard root key C" and paste everything between the lines:
-----------------------------------------------------------------------------------------------------------------
EOF
cat "$tmp/c.key"
cat <<EOF
-----------------------------------------------------------------------------------------------------------------
EOF
if command -v pbpaste >/dev/null; then
	printf 'When it is saved, open the note, copy its whole text again (to check it was saved whole) and press Enter: '
	read -r _
	echo '{}' > "$tmp/check.json"
	signer=$(pbpaste | "$cyx" yard sign root "$tmp/check.json" --key - | sed -n 's/^Signed .* as \([0-9a-f]*\);.*/\1/p')
	if [ "$signer" != "$pub_c" ]; then
		echo "The copied text is not key C. Check the note and run the ceremony again (nothing was kept)." >&2
		exit 1
	fi
	echo "Key C is in Bitwarden."
else
	printf 'When it is saved in Bitwarden, type saved: '
	read -r ans
	[ "$ans" = saved ] || { echo "Not saved: nothing was kept. Run the ceremony again." >&2; exit 1; }
fi
rm -f "$tmp/c.key"
clear 2>/dev/null || true

pub_a=$("$cyx" yard keygen "$key_a" | public)
pub_b=$("$cyx" yard keygen "$key_b" | public)

record=yard-ceremony-record.txt
cat > "$record" <<EOF
Codeyard extension registry: root key ceremony
Date: $(date -u '+%Y-%m-%d %H:%M UTC')
Threshold: 2 of 3

A  $pub_a  encrypted USB stick A ($key_a)
B  $pub_b  encrypted USB stick B ($key_b), kept in another place
C  $pub_c  Bitwarden secure note "Codeyard Yard root key C"

On the server:
  sudo -u yard yard init /srv/yard --registry REGISTRY-DOMAIN --root-keys $pub_a,$pub_b,$pub_c --threshold 2

In the app (crates/app/src/ext_registry.rs, PUBLIC): the three keys above, threshold 2.
EOF
cat "$record"
cat <<EOF

Wrote $record (public keys only; commit it or keep it with the release notes).
Eject both sticks and store B away from A. Sign root.json with two of the keys:
  cyx yard sign root root.json --key "$key_a"
  pbpaste | cyx yard sign root root.json --key -      (key C copied from Bitwarden)
EOF
