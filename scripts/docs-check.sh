#!/usr/bin/env bash
# Keep the docs from contradicting the code: budgets, protocol version, env vars, links, changelog.
#   scripts/docs-check.sh
# The key table and the settings reference are checked by `cargo test -p app docs_`; this script only reads files.
set -euo pipefail
cd "$(dirname "$0")/.."
exec python3 -I - <<'PY'
import glob, os, re, subprocess, sys

errors = []
def fail(msg): errors.append(msg)
def read(p): return open(p, encoding="utf-8").read()

arch = read("docs/architecture.md")
mem = read("scripts/memcheck.sh")

# 1. Memory budgets: docs/architecture.md table == scripts/memcheck.sh
budgets = {m.group(1): int(m.group(2)) for m in re.finditer(r'^measure "([^"]+)"\s+(\d+)', mem, re.M)}
rows = {  # memcheck.sh label -> docs label
    "terminal open": "terminal open",
    "empty window (welcome screen)": "empty window (welcome screen)",
    "small Rust file (360 KB, in a repo)": "small Rust file (360 KB, in a repo)",
    "10k lines, plain text (507 KB)": "10k lines, plain text",
    "10k lines, syntax highlighting": "10k lines, syntax highlighting",
}
for tr, en in rows.items():
    want = budgets.get(tr)
    m = re.search(r"\|\s*" + re.escape(en) + r"\s*\|\s*(\d+) MB\s*\|", arch)
    if want is None: fail(f"memcheck.sh has no budget row '{tr}'")
    elif not m: fail(f"docs/architecture.md has no budget row '{en}'")
    elif int(m.group(1)) != want: fail(f"budget '{en}': docs say {m.group(1)} MB, memcheck.sh says {want} MB")
# binary and agent sizes
mb = re.search(r"^check \"app binary\" (\d+)", mem, re.M)
d = re.search(r"\|\s*app binary\s*\|\s*(\d+) MB", arch)
if not (mb and d): fail("could not find the app binary budget in memcheck.sh or docs/architecture.md")
elif mb.group(1) != d.group(1): fail(f"app binary budget: docs {d.group(1)} MB, memcheck.sh {mb.group(1)} MB")
ab = re.findall(r'check "agent \(\$t\)" (\d+)', mem)
d = re.search(r"agent binary \(Linux musl\)\s*\|\s*(\d+) KB", arch)
if not (ab and d): fail("could not find the agent size budget in memcheck.sh or docs/architecture.md")
elif ab[0] != d.group(1): fail(f"agent budget: docs {d.group(1)} KB, memcheck.sh {ab[0]} KB")
dm = re.search(r'^check "remote agent daemon \(idle\)" (\d+)', mem, re.M)
d = re.search(r"remote agent daemon, idle\s*\|\s*(\d+) MB", arch)
if not (dm and d): fail("could not find the agent daemon budget in memcheck.sh or docs/architecture.md")
elif dm.group(1) != d.group(1): fail(f"agent daemon budget: docs {d.group(1)} MB, memcheck.sh {dm.group(1)} MB")
ci = read(".github/workflows/ci.yml")
m = re.search(r"-le (\d+)", ci); d = re.search(r"agent binary \(Linux musl\)\s*\|\s*(\d+) KB", arch)
if not (m and d): fail("could not find the agent size limit (`-le N`) in .github/workflows/ci.yml or docs/architecture.md")
elif m.group(1) != d.group(1): fail(f"agent size limit: CI {m.group(1)} KB, docs {d.group(1)} KB")

# 2. Protocol version
vm = re.search(r"pub const VERSION: u32 = (\d+);", read("crates/proto/src/lib.rs"))
m = re.search(r"Current protocol version: \*\*(\d+)\*\*", read("docs/remote.md"))
if not vm: fail("crates/proto/src/lib.rs: `pub const VERSION: u32 = N;` not found")
elif not m or m.group(1) != vm.group(1): fail(f"docs/remote.md protocol version {m.group(1) if m else '?'} != proto::VERSION {vm.group(1)}")
dm = re.search(r"GRACE: Duration = Duration::from_secs\((\d+)\)", read("crates/agent/src/daemon.rs"))
if not dm: fail("crates/agent/src/daemon.rs: `GRACE: Duration = Duration::from_secs(N)` not found")
elif f"`GRACE` ({int(dm.group(1))//60} minutes)" not in read("docs/remote.md"): fail("docs/remote.md: daemon GRACE does not match crates/agent/src/daemon.rs")

# 3. arch-check limits
ac = read("scripts/arch-check.sh")
lim = {n: int(x) for n, x in re.findall(r"check crates/app/src/(workbench\.rs|editor\.rs) (\d+)", ac)}
text = " ".join(arch.split())
for f, x in lim.items():
    if f"`{f}` ≤ {x}" not in text: fail(f"docs/architecture.md: size limit of {f} is not {x} (arch-check.sh)")
other = re.search(r"\*\) check \"\$f\" (\d+)", ac); tests = re.search(r"qa_\*_tests\.rs\) check \"\$f\" (\d+)", ac)
helpers = re.search(r"\*_tests\.rs\|\*/test_\*\.rs\) check \"\$f\" (\d+)", ac)
flat = " ".join(text.split())
if not (other and tests and helpers): fail("scripts/arch-check.sh: generic file size limits (`check \"$f\" N`) not found")
elif f"every other source file ≤ {other.group(1)} (`*_tests.rs` and test helpers ≤ {helpers.group(1)}, `tests.rs` and `qa_*_tests.rs` ≤ {tests.group(1)})" not in flat:
    fail("docs/architecture.md: generic file size limits differ from arch-check.sh")
st = re.search(r'"\$n" -gt (\d+) \]; then echo "FAIL Workbench', ac)
if not st: fail("scripts/arch-check.sh: Workbench struct limit not found")
elif f"`Workbench` struct ≤ {st.group(1)} lines" not in text: fail(f"docs/architecture.md: Workbench struct limit is not {st.group(1)} lines (arch-check.sh)")

# 4. Environment variables
used = set()
for p in glob.glob("crates/**/*.rs", recursive=True) + glob.glob("scripts/*"):
    if "/grammars/" in p or os.path.isdir(p): continue
    used |= set(re.findall(r"CODEYARD_[A-Z0-9_]+", read(p)))
used -= {"CODEYARD_NO_SUCH_VAR"}  # only a test input
env_doc = read("docs/environment.md")
for name in sorted(used):
    if name not in env_doc: fail(f"{name} is read by the code but not documented in docs/environment.md")
for name in sorted(set(re.findall(r"CODEYARD_[A-Z0-9_]+", env_doc)) - used):
    fail(f"{name} is documented in docs/environment.md but no code reads it")

# 5. Relative links and the document index
def prose(p): return re.sub(r"^(```|~~~).*?^\1.*?$", "", read(p), flags=re.S | re.M)  # without fenced code blocks
md = (["README.md", "CONTRIBUTING.md", "SECURITY.md", "CHANGELOG.md", "CODE_OF_CONDUCT.md", "THIRD_PARTY_NOTICES.md", "vendor/gpui/PATCHES.md"]
      + sorted(glob.glob("docs/*.md")) + sorted(glob.glob(".github/*.md")))
for p in md:
    base = os.path.dirname(p)
    body = prose(p)
    # inline links `[x](target#frag)`, `[x](#frag)` (same file) and reference definitions `[x]: target`
    links = [(m.group(1), m.group(2)) for m in re.finditer(r"\]\(([^)#\s]*)(#[^)]*)?\)", body)]
    links += [(m.group(1), m.group(2)) for m in re.finditer(r"^\[[^\]]+\]:\s*([^#\s]+)(#\S*)?\s*$", body, re.M)]
    for t, frag in links:
        if "://" in t or t.startswith("mailto:") or (not t and not frag): continue
        target = os.path.normpath(os.path.join(base, t)) if t else p
        if not os.path.exists(target): fail(f"{p}: broken link {t}"); continue
        if frag and target.endswith(".md"):
            heads = {re.sub(r"[^\w\- ]", "", h.strip().lower()).replace(" ", "-") for h in re.findall(r"^#+ (.+)$", prose(target), re.M)}
            if frag[1:] not in heads: fail(f"{p}: link {t}{frag} points to a heading that does not exist")
index = read("docs/README.md")
for p in sorted(glob.glob("docs/*.md")):
    n = os.path.basename(p)
    if n != "README.md" and f"]({n})" not in index: fail(f"docs/README.md does not list {n}")

# 5b. The user guide's table of contents must match its headings
def anchor(h): return re.sub(r"[^\w\- ]", "", h.strip().lower()).replace(" ", "-")
guide = read("docs/user-guide.md")
toc = re.search(r"<!-- toc -->\n(.*?)\n<!-- /toc -->", guide, re.S)
body = re.sub(r"```.*?```", "", guide[toc.end():], flags=re.S) if toc else guide
want = [f"- [{h}](#{anchor(h)})" for h in re.findall(r"^## (.+)$", body, re.M)]
if not toc or toc.group(1).split("\n") != want: fail("docs/user-guide.md: the table of contents does not match its ## headings")

# 6. Changelog: version, tags, links
cl = read("CHANGELOG.md")
ver = re.search(r'^version = "([^"]+)"', read("Cargo.toml"), re.M).group(1)
if f"## [{ver}]" not in cl: fail(f"CHANGELOG.md has no section for the current version {ver}")
if "## [Unreleased]" not in cl: fail("CHANGELOG.md has no Unreleased section")
heads = re.findall(r"^## \[(\d+\.\d+\.\d+)\]", cl, re.M)
for h in heads:
    if not re.search(rf"^\[{re.escape(h)}\]: ", cl, re.M): fail(f"CHANGELOG.md: no link reference for {h}")
gt = subprocess.run(["git", "tag"], capture_output=True, text=True)
if gt.returncode != 0: print("docs-check: note: `git tag` failed, tag/CHANGELOG check skipped:", gt.stderr.strip(), file=sys.stderr)
for t in gt.stdout.split():
    if t.startswith("v") and t[1:] not in heads: fail(f"git tag {t} has no CHANGELOG section")

# 6b. Licence files must match the declaration in Cargo.toml
lic = re.search(r'^license = "([^"]+)"', read("Cargo.toml"), re.M).group(1)
for spdx, f in [("MIT", "LICENSE-MIT"), ("Apache-2.0", "LICENSE-APACHE")]:
    if spdx in lic and not os.path.exists(f): fail(f"Cargo.toml declares {spdx} but {f} is missing")
if "MIT OR Apache-2.0" not in lic: fail(f"Cargo.toml licence is '{lic}'; README, CONTRIBUTING and LICENSE files assume MIT OR Apache-2.0")

# 7. Command-line flags: README == main.rs --help
main = read("crates/app/src/main.rs")
usage = re.search(r'println!\(\s*"usage: ([^"]+)"', main)
if not usage: fail('crates/app/src/main.rs: the `println!("usage: ...")` help text was not found')
else:
    flags = set(re.findall(r"--[a-z][a-z-]*", usage.group(1)))
    quick = read("README.md")
    for f in sorted(flags):
        if f not in quick: fail(f"README.md does not mention the {f} flag (main.rs --help)")
    # every flag the parser accepts is in --help, except the hidden test flag and the standard ones
    parsed = set(re.findall(r'^\s*(?:"-[A-Za-z]" \| )?"(--[a-z][a-z-]*)"(?: \| "-?-?[A-Za-z-]+")* =>', main, re.M))
    if not parsed: fail("crates/app/src/main.rs: no `\"--flag\" =>` arms found (flag check is stale)")
    for f in sorted(parsed - flags - {"--do", "--help", "--version"}):
        fail(f"crates/app/src/main.rs parses {f} but --help does not list it")

if errors:
    print("docs-check: FAIL")
    for e in errors: print("  -", e)
    sys.exit(1)
print("docs-check: ok")
PY
