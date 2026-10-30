# Exthost spike: measurements and decisions

The Phase 2 week-1 spike (#97, [plugin-architecture.md §14](plugin-architecture.md#14-roadmap)): what an exthost
costs on real builds, measured before `codeyard-exthost` is designed, and the decisions that follow. The harness is
[scripts/perf/exthost-spike/](../scripts/perf/exthost-spike/README.md); nothing from it ships.

## Decisions

1. **TypeScript: go.** It ships with Rust in Phase 2 (#107). Cold activation is 20 ms p95 against a 150 ms ceiling.
   Memory is higher than Rust but stable and predictable (see 2).
2. **TypeScript memory ceiling: 50 MB idle private memory per extension** (macOS; measured 43 MB for an Octokit + zod
   extension, 40 MB for hello). About 32 MB of it is the JavaScript engine's machine code, which every TypeScript
   extension carries and which macOS does not let processes share (see [macOS](#macos-executable-memory)). On Linux
   and Windows the same code is file-mapped and shared, so the cost is far lower.
3. **No shared process.** Rust measures 4.8 MB (hello) and 7.8 MB (a 1.6 MB component) against the 25 MB ceiling, so
   §15 decision 2 does not trigger: one process per active extension stays. Sharing a process among network-less
   TypeScript extensions would save about 32 MB per extra extension; it stays in [Later](plugin-architecture.md#13-later).
4. **macOS: one hardened-runtime exception, on the exthost only:** `com.apple.security.cs.allow-unsigned-executable-memory`
   ([exthost.entitlements](../scripts/perf/exthost-spike/exthost.entitlements)). `allow-jit` is not enough. The
   exthost loads compiled code by copying only its text into anonymous memory (a wasmtime `CustomCodeMemory`); data
   and linear-memory images stay file-mapped.
5. **The OS memory cap cannot be `RLIMIT_AS` as configured today:** wasmtime reserves about 391 GB of address space
   (guard regions) for one instance. #99 either shrinks `memory_reservation` and guard sizes until an address-space
   cap fits, or caps resident memory another way (Job Object commit limit on Windows; on Linux and macOS, the host
   checks resident size).
6. **Epoch ticks only while guest code runs.** A ticker every 10 ms in an idle process is 100 wakeups a second; the
   exthost starts it on call entry and stops it on return.
7. **Hot reload uses Cranelift's incremental cache in `cyx dev`.** Every TypeScript build embeds the same engine, so
   the cache halves a TypeScript reload (about 1 s → 0.5 s). Install-time compiles do not use it.
8. **Perf table:** "Exthost memory per extension" gets a TypeScript ceiling of 50 MB; a "Hot reload, TypeScript"
   row is added with a 750 ms ceiling (measured 527 ms p95). Rust rows keep their ceilings.
9. **Notarization moves to #99.** The spike reproduces the hardened runtime with ad hoc signing, which enforces the
   same executable-memory rules; notarizing a binary that does not ship would prove nothing more. #99 ships the
   exthost in the bundle with these entitlements and verifies it on a notarized dry run.

## Results

Reference Mac (Apple M1 Max, 32 GB, macOS 27.2), release build, wasmtime 50.0.0, 20 runs. The shipping setup:
hardened runtime, the exthost entitlement, text copied to anonymous memory, deny-default Seatbelt profile.

| Component | .wasm | .cwasm | Compile (median) | Cold start p50 / p95 | Warm call p50 / p95 | Idle private |
| --- | --- | --- | --- | --- | --- | --- |
| Rust hello | 0.05 MB | 0.21 MB | 14 ms | 9.6 / 10.1 ms | 0.005 / 0.008 ms | 4.8 MB |
| Rust, 1.6 MB component | 1.60 MB | 3.39 MB | 151 ms | 10.9 / 11.5 ms | 0.008 / 0.013 ms | 7.8 MB |
| TypeScript hello | 11.99 MB | 33.13 MB | 1022 ms | 18.1 / 20.0 ms | 0.021 / 0.028 ms | 40.0 MB |
| TypeScript, Octokit + zod | 18.64 MB | 38.37 MB | 1029 ms | 19.7 / 20.2 ms | 0.033 / 0.040 ms | 43.0 MB |

| Hot reload | From .wasm p50 / p95 | From .cwasm p50 / p95 |
| --- | --- | --- |
| Rust hello | 5 / 7 ms | 0.2 / 0.2 ms |
| Rust, 1.6 MB component | 59 / 74 ms | 2.1 / 2.4 ms |
| TypeScript hello, alternating with an edited build | 458 / 506 ms | 7.5 / 7.8 ms |
| TypeScript, Octokit + zod | 454 / 527 ms | 9.3 / 9.7 ms |

Repeated runs reproduced every row within 10%.

Cold start is spawn → load → instantiate → `activate()` → first reply. "Idle private" is `phys_footprint`, what
Activity Monitor shows as Memory. Warm calls are stdio round trips, so the IPC itself costs microseconds, well
inside the 3 ms provider-overhead ceiling.

### Where the time and memory go

The same Mac without the hardened runtime and Seatbelt (code file-mapped, as `cyx dev` runs the author's own folder
on any platform without an OS sandbox):

| Component | Cold start p50 / p95 | Idle private |
| --- | --- | --- |
| Rust hello | 2.7 / 3.0 ms | 2.0 MB |
| Rust, 1.6 MB component | 4.3 / 4.6 ms | 3.1 MB |
| TypeScript hello | 7.9 / 8.5 ms | 5.6 MB |

Seatbelt adds about 6 ms and 2.7 MB. Copying the text to anonymous memory adds its size: 32 MB for TypeScript,
under 4 MB for Rust.

### macOS executable memory

Under the hardened runtime, which notarization requires:

| Entitlement | Compile in process | Load a file-mapped .cwasm | Load a .cwasm copied to anonymous memory (whole / text only) |
| --- | --- | --- | --- |
| none | killed (SIGKILL) | refused (`EACCES`) | killed / killed |
| `allow-jit` | killed | refused | killed / killed |
| `allow-unsigned-executable-memory` | works | refused | works / works |
| `disable-executable-page-protection` | works | refused | works / works |

`allow-jit` covers only `MAP_JIT` memory, which wasmtime does not use. No entitlement makes a file mapping
executable. `disable-executable-page-protection` is broader than needed and gains nothing. Copying the whole
`.cwasm` (wasmtime's `Component::deserialize`, `SPIKE_LOAD=bytes`) works, but copies data and linear-memory images
too: TypeScript hello costs 75 MB that way, against 40 MB when only the text is copied.
Mapping the text from a POSIX shared memory object, so that TypeScript extensions share one copy, fails with a
protection fault even without the hardened runtime.

### Linux and Windows

GitHub-hosted runners (`ubuntu-latest` x86_64, `windows-latest` x86_64; shared and noisy, so informational only),
unsandboxed, code file-mapped, from the `exthost-spike` workflow on this PR:

| Component | Linux cold start p50 / p95 | Linux idle private | Windows cold start p50 / p95 | Windows idle private |
| --- | --- | --- | --- | --- |
| Rust hello | 2.1 / 2.3 ms | 12.5 MB | 5.1 / 5.5 ms | 0.7 MB |
| Rust, 1.6 MB component | 4.0 / 4.9 ms | 1.6 MB | 6.9 / 7.2 ms | 2.1 MB |
| TypeScript hello | 4.3 / 5.3 ms | 2.5 MB | 12.0 / 13.6 ms | 9.4 MB |
| TypeScript, Octokit + zod | 7.7 / 8.6 ms | 4.2 MB | 16.0 / 17.0 ms | 14.4 MB |

Private memory is private dirty + swap on Linux and the private working set on Windows. Both systems map compiled
code from the file and share it, so TypeScript costs a fraction of what it does on macOS. (The Linux Rust hello row
is higher than the larger Rust component in this run; #99 looks at it with the real host.) These runners have 3–4
cores against the reference Mac's 10, so Cranelift is 3–5 times slower there: a TypeScript hot reload takes 1.2–1.6 s
p95 and a Rust 1.6 MB one 165–200 ms.

### Hot reload in the app (#106)

Measured after the spike with the real dev window: a new `extension.wasm` in place → the old exthost stopped
(`deactivate` saves `ctx.state`) → the new build compiled with the incremental cache and started with the reason
`reload` → the first command answered, `ctx.state` carried over. The `cyx dev` rebuild (cargo) is not included, nor
the dev window's 100 ms poll for the new file. Reference Mac, release build of the app's tests and of `codeyard-exthost`
(`ext_dev_tests::reload_time`, 20 runs alternating two builds that differ in one constant):

| Component | p50 | p95 | Of which the compile |
| --- | --- | --- | --- |
| Rust, 0.22 MB (`ctx.state` counter) | 30 ms | 33 ms | about 25 ms |
| Rust, 1.9 MB (the same with regex, toml and pulldown-cmark) | 168 ms | 180 ms | about 160 ms |
| TypeScript, 12 MB (the same counter) | 616 ms | 655 ms | about 550 ms |

Stopping the old exthost (its `deactivate` included) takes about 9 ms. For Rust components the incremental cache
saves little (a changed constant moves code that LTO inlined everywhere: 219 ms against 251 ms without it, compiling
alone); it pays for TypeScript, where the engine is the same in every build.

## What the spike did not cover

- Linux and Windows sandboxes (#100): their cost is measured there.
- WebAssembly-level `fetch`, processes and file access: they go through the broker (#101, #102), and the warm-call
  numbers bound the IPC cost.
- ComponentizeJS's ahead-of-time mode (weval): it trades a larger component for faster JavaScript, which matters only
  if TypeScript provider latency misses its ceiling.
