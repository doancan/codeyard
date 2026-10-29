# Exthost spike

The Phase 2 week-1 spike (#97): a throwaway exthost and a few test extensions, measured before `codeyard-exthost`
is designed. Results and decisions are in [docs/extensions-spike.md](../../../docs/extensions-spike.md). Nothing here
ships, and nothing here is a workspace member: every folder is its own cargo project, so the app's dependency tree
never sees wasmtime.

```
scripts/perf/exthost-spike/run.sh [out-dir]
```

Needs the `wasm32-wasip2` Rust target and Node.js. It builds everything, then prints two Markdown tables.
`.github/workflows/exthost-spike.yml` runs it on Linux, Windows and macOS runners.

| Path | What it is |
| --- | --- |
| `wit/spike.wit` | A two-function world (`activate`, `call`) with one host import (`log`) |
| `guest-rust/` | Rust hello, built with plain cargo for `wasm32-wasip2` |
| `guest-rust-big/` | A 1.6 MB Rust component (regex, serde, toml, chrono, url): an integration extension's weight |
| `guest-js/` | `hello.js` and `octokit.js` (Octokit + zod, bundled with esbuild), componentized with ComponentizeJS |
| `host/` | `spike-exthost` (`compile` and `run` modes) and `spike-measure` (the driver) |
| `exthost.entitlements` | The macOS hardened-runtime exception the exthost needs |

What `spike-measure` reports, per component, 20 runs unless noted:
- **Compile:** `spike-exthost compile` (Cranelift, `.wasm` → `.cwasm`), median of 5.
- **Cold start:** spawn → `ready` (load `.cwasm`, instantiate, `activate()`) → first reply.
- **Warm call:** 2000 round trips over stdio in one process.
- **Idle memory:** after 2 s idle. *Private* is what the process alone costs: `phys_footprint` on macOS (Activity
  Monitor's Memory), private dirty + swap on Linux, the private working set on Windows.
- **Hot reload:** in one process, load and activate a new instance and drop the old one, then call it. From `.wasm`
  (Cranelift included, with the incremental cache `cyx dev` would use; the first reload is left out) and from `.cwasm`.
  `js-hello` alternates with an edited build.

On macOS `run.sh` reproduces the shipping setup: the exthost is signed ad hoc with the hardened runtime and
`exthost.entitlements`, loads code with `SPIKE_LOAD=copy-text` and runs under a deny-default Seatbelt profile
(`SPIKE_SANDBOX=1`). Without those variables the same binary measures the unsandboxed, file-mapped case;
`SPIKE_LOAD=bytes` copies the whole `.cwasm` into anonymous memory instead of only its text.
