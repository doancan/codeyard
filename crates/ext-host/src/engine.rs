//! The wasmtime engine, compiling ahead of time, and loading compiled code.
//!
//! On macOS the hardened runtime lets only anonymous memory become executable (with
//! `com.apple.security.cs.allow-unsigned-executable-memory`, the exthost's one exception), never a file mapping. So
//! compiled code is file-mapped as usual and only its text is copied into anonymous memory (`CopyText`); data and
//! linear-memory images stay file-backed and shared. docs/extensions-spike.md has the measurements.

use std::path::Path;

use sha2::{Digest, Sha256};
use wasmtime::component::Component;
use wasmtime::{Config, Engine, Result, WasmBacktraceDetails, format_err};

fn config() -> Config {
    let mut c = Config::new();
    c.wasm_component_model(true);
    c.epoch_interruption(true);
    c.wasm_backtrace_details(WasmBacktraceDetails::Enable);
    #[cfg(unix)]
    c.with_custom_code_memory(Some(std::sync::Arc::new(CopyText)));
    c
}

pub fn engine() -> Result<Engine> {
    Engine::new(&config())
}

/// `.wasm` → compiled code, for the app's cache. With `cache`, Cranelift reuses the functions it compiled last time
/// (`incremental.rs`); the result is the same code, and loads in an engine without the cache.
pub fn compile(wasm: &[u8], cache: Option<&Path>) -> Result<Vec<u8>> {
    let Some(path) = cache else { return engine()?.precompile_component(wasm) };
    let store = std::sync::Arc::new(crate::incremental::FileCache::open(path));
    let mut c = config();
    c.enable_incremental_compilation(store.clone())?;
    let code = Engine::new(&c)?.precompile_component(wasm)?;
    store.save();
    Ok(code)
}

/// Load compiled code after checking it is the file the app compiled for this extension: the cache is shared, so a
/// mismatch must not run another extension's code.
pub fn load(engine: &Engine, path: &Path, sha256: &[u8; 32]) -> Result<Component> {
    // Hashed in pieces: a whole TypeScript .cwasm read at once is 33 MB that the allocator keeps, dirty, after it is
    // freed, which macOS counts as the process's memory.
    let mut file = std::fs::File::open(path).map_err(|e| format_err!("cannot read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = std::io::Read::read(&mut file, &mut buf).map_err(|e| format_err!("cannot read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let have: [u8; 32] = hasher.finalize().into();
    if &have != sha256 {
        return Err(format_err!("{} does not match its recorded hash", path.display()));
    }
    // Safety: the file is the one the app compiled with this engine (checked above); `deserialize_file` maps it and
    // checks the engine configuration and version.
    unsafe { Component::deserialize_file(engine, path) }
}

#[cfg(unix)]
struct CopyText;

#[cfg(unix)]
fn page() -> usize {
    // Safety: a plain libc query.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

#[cfg(unix)]
impl wasmtime::CustomCodeMemory for CopyText {
    fn required_alignment(&self) -> usize {
        page()
    }

    fn publish_executable(&self, ptr: *const u8, len: usize) -> Result<()> {
        let len = len.div_ceil(page()) * page();
        // Safety: wasmtime hands over a page-aligned range it owns and that no code runs from yet. The copy is
        // taken before the range is replaced, in its own mapping that is unmapped right after: a heap copy (32 MB for
        // TypeScript) would stay with the allocator, dirty, after it is freed.
        unsafe {
            let rw = libc::PROT_READ | libc::PROT_WRITE;
            let copy = libc::mmap(std::ptr::null_mut(), len, rw, libc::MAP_PRIVATE | libc::MAP_ANON, -1, 0);
            if copy == libc::MAP_FAILED {
                return Err(format_err!("mmap: {}", std::io::Error::last_os_error()));
            }
            std::ptr::copy_nonoverlapping(ptr, copy as *mut u8, len);
            let at = ptr as *mut libc::c_void;
            let flags = libc::MAP_FIXED | libc::MAP_PRIVATE | libc::MAP_ANON;
            let p = libc::mmap(at, len, rw, flags, -1, 0);
            if p != at {
                libc::munmap(copy, len);
                return Err(format_err!("mmap: {}", std::io::Error::last_os_error()));
            }
            std::ptr::copy_nonoverlapping(copy as *const u8, p as *mut u8, len);
            libc::munmap(copy, len);
            if libc::mprotect(p, len, libc::PROT_READ | libc::PROT_EXEC) != 0 {
                return Err(format_err!("mprotect: {}", std::io::Error::last_os_error()));
            }
            #[cfg(target_os = "macos")]
            sys_icache_invalidate(p, len);
            #[cfg(not(target_os = "macos"))]
            clear_cache(p as *mut u8, len);
        }
        Ok(())
    }

    fn unpublish_executable(&self, ptr: *const u8, len: usize) -> Result<()> {
        let len = len.div_ceil(page()) * page();
        // Safety: as above; no code runs from the range any more.
        if unsafe { libc::mprotect(ptr as *mut _, len, libc::PROT_READ | libc::PROT_WRITE) } != 0 {
            return Err(format_err!("mprotect: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn sys_icache_invalidate(start: *mut libc::c_void, len: usize);
}

/// x86-64 keeps instruction caches coherent; aarch64 Linux needs them cleaned (the compiler builtin does that).
#[cfg(all(unix, not(target_os = "macos")))]
unsafe fn clear_cache(start: *mut u8, len: usize) {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe extern "C" {
            fn __clear_cache(start: *mut libc::c_char, end: *mut libc::c_char);
        }
        // Safety: the range was just written and is mapped.
        unsafe { __clear_cache(start as *mut libc::c_char, start.add(len) as *mut libc::c_char) };
    }
    #[cfg(not(target_arch = "aarch64"))]
    let _ = (start, len);
}
