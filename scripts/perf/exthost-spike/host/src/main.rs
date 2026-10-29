//! Throwaway exthost for the Phase 2 spike (#97). Not shipped; see ../README.md.
//!
//!   spike-exthost compile < in.wasm > out.cwasm
//!   spike-exthost run <file.cwasm|file.wasm>
//!
//! `run` prints `ready` after `activate()`, then answers one command per stdin line:
//! `call <text>` → `ok <reply>`, `reload <file>` → `reloaded <micros>`, `quit`.

use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use wasmtime::{Result, bail, format_err};
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

wasmtime::component::bindgen!({ path: "../wit", world: "ext" });

struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl codeyard::spike::host::Host for State {
    fn log(&mut self, msg: String) {
        eprintln!("guest: {msg}");
    }
}

fn engine(dev: bool) -> Result<Engine> {
    let mut c = Config::new();
    c.wasm_component_model(true);
    c.epoch_interruption(true);
    // Hot reload: Cranelift reuses the code of functions it compiled before in this process. Every TypeScript build
    // embeds the same engine, so a reload recompiles only what changed. Install-time `compile` does not need it.
    if dev {
        c.enable_incremental_compilation(std::sync::Arc::new(Incremental::default()))?;
    }
    #[cfg(unix)]
    if std::env::var_os("SPIKE_LOAD").is_some_and(|v| v == "copy-text") {
        c.with_custom_code_memory(Some(std::sync::Arc::new(CopyText)));
    }
    Engine::new(&c)
}

/// Under the macOS hardened runtime only anonymous memory may become executable (with
/// `com.apple.security.cs.allow-unsigned-executable-memory`), never a file mapping. Replacing just the text pages
/// of a file-mapped `.cwasm` with an anonymous copy keeps the data (and linear-memory images) file-backed, so only
/// the machine code counts as dirty memory.
#[derive(Debug, Default)]
struct Incremental(std::sync::Mutex<std::collections::HashMap<Vec<u8>, Vec<u8>>>);

impl wasmtime::CacheStore for Incremental {
    fn get(&self, key: &[u8]) -> Option<std::borrow::Cow<'_, [u8]>> {
        self.0.lock().unwrap().get(key).cloned().map(std::borrow::Cow::Owned)
    }

    fn insert(&self, key: &[u8], value: Vec<u8>) -> bool {
        self.0.lock().unwrap().insert(key.to_vec(), value);
        true
    }
}

#[cfg(unix)]
struct CopyText;

#[cfg(unix)]
fn page() -> usize {
    // Safety: plain libc query.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

#[cfg(unix)]
impl wasmtime::CustomCodeMemory for CopyText {
    fn required_alignment(&self) -> usize {
        page()
    }

    fn publish_executable(&self, ptr: *const u8, len: usize) -> Result<()> {
        let len = len.div_ceil(page()) * page();
        // Safety: wasmtime hands us a page-aligned range it owns and that no code runs from yet.
        unsafe {
            let copy = std::slice::from_raw_parts(ptr, len).to_vec();
            let at = ptr as *mut libc::c_void;
            let flags = libc::MAP_FIXED | libc::MAP_PRIVATE | libc::MAP_ANON;
            let p = libc::mmap(at, len, libc::PROT_READ | libc::PROT_WRITE, flags, -1, 0);
            if p != at {
                bail!("mmap: {}", std::io::Error::last_os_error());
            }
            std::ptr::copy_nonoverlapping(copy.as_ptr(), p as *mut u8, len);
            if libc::mprotect(p, len, libc::PROT_READ | libc::PROT_EXEC) != 0 {
                bail!("mprotect: {}", std::io::Error::last_os_error());
            }
            #[cfg(target_os = "macos")]
            sys_icache_invalidate(p, len);
        }
        Ok(())
    }

    fn unpublish_executable(&self, ptr: *const u8, len: usize) -> Result<()> {
        let len = len.div_ceil(page()) * page();
        // Safety: as above; nothing runs from the range any more.
        if unsafe { libc::mprotect(ptr as *mut _, len, libc::PROT_READ | libc::PROT_WRITE) } != 0 {
            bail!("mprotect: {}", std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn sys_icache_invalidate(start: *mut libc::c_void, len: usize);
    fn sandbox_init(profile: *const libc::c_char, flags: u64, errorbuf: *mut *mut libc::c_char) -> libc::c_int;
}

/// Seatbelt with the exthost policy (docs/plugin-architecture.md §3): no network, no exec, no writes, reads only
/// the extension's compiled code. A stand-in for #100; it measures the cost and checks that wasmtime still works.
#[cfg(target_os = "macos")]
fn sandbox(path: &Path) -> Result<()> {
    let dir = std::fs::canonicalize(path)?.parent().unwrap().to_owned();
    let profile = format!(
        r#"(version 1)
(deny default)
(allow file-read* (subpath "{}"))
(allow file-read-metadata)
(allow sysctl-read)
(allow process-info* (target self))
(allow signal (target self))"#,
        dir.display()
    );
    let c = std::ffi::CString::new(profile).unwrap();
    let mut err = std::ptr::null_mut();
    // Safety: SBPL string, error buffer owned by libsandbox.
    if unsafe { sandbox_init(c.as_ptr(), 0, &mut err) } != 0 {
        bail!("sandbox_init failed");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn sandbox(_: &Path) -> Result<()> {
    Ok(())
}

fn load(engine: &Engine, path: &Path) -> Result<Component> {
    if path.extension().is_some_and(|e| e == "cwasm") {
        // Safety: the spike only loads files it compiled itself (the real host checks a sha256 sidecar first).
        // Under the macOS hardened runtime a file mapping cannot become executable, so SPIKE_LOAD=bytes copies the
        // code into anonymous memory first (see ../README.md).
        if std::env::var_os("SPIKE_LOAD").is_some_and(|v| v == "bytes") {
            unsafe { Component::deserialize(engine, std::fs::read(path)?) }
        } else {
            unsafe { Component::deserialize_file(engine, path) }
        }
    } else {
        Component::from_file(engine, path)
    }
}

struct Instance {
    store: Store<State>,
    ext: Ext,
}

fn start(engine: &Engine, linker: &Linker<State>, path: &Path) -> Result<Instance> {
    let component = load(engine, path).map_err(|e| e.context(format!("loading {}", path.display())))?;
    let state = State {
        wasi: WasiCtx::builder().build(),
        table: ResourceTable::new(),
        limits: StoreLimitsBuilder::new().memory_size(256 << 20).instances(16).build(),
    };
    let mut store = Store::new(engine, state);
    store.limiter(|s| &mut s.limits);
    store.set_epoch_deadline(1);
    store.epoch_deadline_callback(|_| Ok(UpdateDeadline::Continue(1)));
    let ext = Ext::instantiate(&mut store, &component, linker)?;
    ext.call_activate(&mut store)?;
    Ok(Instance { store, ext })
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let engine = engine(args.get(1).is_some_and(|a| a == "run"))?;
    match args.get(1).map(String::as_str) {
        Some("compile") => {
            let mut wasm = Vec::new();
            std::io::stdin().read_to_end(&mut wasm)?;
            let code = engine.precompile_component(&wasm)?;
            std::io::stdout().write_all(&code)?;
            Ok(())
        }
        Some("run") => run(&engine, Path::new(args.get(2).ok_or_else(|| format_err!("run <file>"))?)),
        _ => bail!("usage: spike-exthost compile|run <file>"),
    }
}

fn run(engine: &Engine, path: &Path) -> Result<()> {
    // Guest code yields to the host every 10 ms (docs/plugin-architecture.md §3).
    let ticker = engine.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(10));
            ticker.increment_epoch();
        }
    });
    if std::env::var_os("SPIKE_SANDBOX").is_some() {
        sandbox(path)?;
    }
    let mut linker = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    Ext::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s)?;
    let mut inst = start(engine, &linker, path)?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "ready")?;
    out.flush()?;
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let (cmd, arg) = line.split_once(' ').unwrap_or((&line, ""));
        match cmd {
            "call" => {
                let reply = inst.ext.call_call(&mut inst.store, arg)?;
                writeln!(out, "ok {reply}")?;
            }
            "reload" => {
                let t = Instant::now();
                let next = start(engine, &linker, Path::new(arg))?;
                inst = next;
                writeln!(out, "reloaded {}", t.elapsed().as_micros())?;
            }
            "quit" => break,
            _ => writeln!(out, "err unknown command")?,
        }
        out.flush()?;
    }
    Ok(())
}
