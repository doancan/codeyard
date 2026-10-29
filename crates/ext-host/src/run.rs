//! The exthost's loop: handshake, load, `activate`, then one guest call per frame from the app. Single-threaded:
//! the app drives everything (timers included), and exports return quickly because I/O finishes through tickets.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use ext_manifest::limits;
use ext_proto::{Reader, ToApp, ToHost, Trap};
use wasmtime::component::{HasSelf, Linker};
use wasmtime::{Engine, Store, StoreLimitsBuilder, UpdateDeadline, WasmBacktrace};

use crate::bindings::Extension;
use crate::convert;
use crate::state::State;

/// Ticks the engine's epoch every 10 ms, but only while guest code runs: an idle exthost causes no wakeups.
struct Ticker {
    busy: Arc<(Mutex<bool>, Condvar)>,
    stop: Arc<AtomicBool>,
}

impl Ticker {
    fn start(engine: &Engine) -> Ticker {
        let busy = Arc::new((Mutex::new(false), Condvar::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (b, s, e) = (busy.clone(), stop.clone(), engine.clone());
        std::thread::spawn(move || {
            let (lock, cv) = &*b;
            while !s.load(Ordering::Relaxed) {
                let guard = lock.lock().unwrap_or_else(|p| p.into_inner());
                let guard = cv.wait_while(guard, |busy| !*busy && !s.load(Ordering::Relaxed)).unwrap_or_else(|p| p.into_inner());
                drop(guard);
                std::thread::sleep(Duration::from_millis(10));
                e.increment_epoch();
            }
        });
        Ticker { busy, stop }
    }

    fn set(&self, on: bool) {
        let (lock, cv) = &*self.busy;
        *lock.lock().unwrap_or_else(|p| p.into_inner()) = on;
        cv.notify_all();
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.set(false);
    }
}

/// Run one extension. Returns when the app sends `Shutdown` or closes the pipe; an `Err` means the exthost should
/// exit with a failure (after a `Trap` frame where there was one).
pub fn run(input: impl Read, output: impl Write + Send + 'static) -> Result<(), String> {
    let mut reader = Reader::new(input);
    let hello = match reader.read::<ToHost>(Instant::now()) {
        Ok(ToHost::Hello(h)) => h,
        Ok(_) => return Err("the first frame was not Hello".into()),
        Err(e) => return Err(e.to_string()),
    };
    let mut output: Box<dyn Write + Send> = Box::new(output);
    if !ext_proto::handshake_ok(&hello, ext_proto::BUILD) {
        let _ = ext_proto::write(&mut output, &ToApp::Mismatch { version: ext_proto::VERSION, build: ext_proto::BUILD.into() });
        return Err(format!("the app is build {} and this exthost {}", hello.build, ext_proto::BUILD));
    }
    ext_proto::write(&mut output, &ToApp::Ready { version: ext_proto::VERSION, build: ext_proto::BUILD.into() }).map_err(|e| e.to_string())?;

    let engine = crate::engine::engine().map_err(|e| e.to_string())?;
    let component = crate::engine::load(&engine, std::path::Path::new(&hello.compiled), &hello.compiled_sha256).map_err(|e| format!("{e:#}"))?;
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| e.to_string())?;
    Extension::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s).map_err(|e| e.to_string())?;

    let store_limits = StoreLimitsBuilder::new().memory_size(limits::GUEST_MEMORY_BYTES).trap_on_grow_failure(true).build();
    let mut store = Store::new(&engine, State::new(output, &hello, store_limits));
    store.limiter(|s| &mut s.limits);
    let ticker = Ticker::start(&engine);
    let mut guest = Guest { store, ticker, ext: None };

    let ext = guest.call(limits::ACTIVATE_MILLIS, |store, _| Extension::instantiate(store, &component, &linker))?;
    guest.ext = Some(ext);
    let activated = guest.call(limits::ACTIVATE_MILLIS, |store, ext| {
        ext.expect("instantiated").codeyard_api_guest().call_activate(store, &convert::activation(&hello.reason))
    })?;
    guest.store.data_mut().send(&ToApp::Activated(activated));
    guest.drain()?;

    loop {
        let msg = match reader.read::<ToHost>(Instant::now()) {
            Ok(m) => m,
            Err(ext_proto::Error::Closed) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        match msg {
            ToHost::Hello(_) => return Err("a second Hello".into()),
            ToHost::Handle { id, request, .. } => {
                let request = request.into();
                guest.call(limits::GUEST_CALL_MILLIS, |s, x| x.expect("instantiated").codeyard_api_guest().call_handle(s, id, &request))?;
            }
            ToHost::Cancel { id } => {
                guest.call(limits::GUEST_CALL_MILLIS, |s, x| x.expect("instantiated").codeyard_api_guest().call_cancel(s, id))?;
            }
            ToHost::Complete { ticket, completion } => guest.complete(ticket, completion.into())?,
            ToHost::Event { ticket, event } => {
                let event = event.into();
                guest.call(limits::GUEST_CALL_MILLIS, |s, x| x.expect("instantiated").codeyard_api_guest().call_event(s, ticket, &event))?;
            }
            ToHost::Settings(values) => guest.store.data_mut().settings.extend(values),
            ToHost::Env(env) => guest.store.data_mut().env = env,
            ToHost::Shutdown => {
                guest.call(limits::GUEST_CALL_MILLIS, |s, x| x.expect("instantiated").codeyard_api_guest().call_deactivate(s))?;
                return Ok(());
            }
        }
        guest.drain()?;
    }
}

struct Guest {
    store: Store<State>,
    ticker: Ticker,
    ext: Option<Extension>,
}

impl Guest {
    /// One call into the guest with a CPU budget. A trap (or the budget running out) is reported to the app as a
    /// `Trap` frame and ends the exthost.
    fn call<R>(&mut self, budget_ms: u64, f: impl FnOnce(&mut Store<State>, Option<&Extension>) -> wasmtime::Result<R>) -> Result<R, String> {
        let started = Instant::now();
        let budget = Duration::from_millis(budget_ms);
        self.store.set_epoch_deadline(1);
        self.store.epoch_deadline_callback(move |_| {
            if started.elapsed() > budget {
                Err(wasmtime::format_err!("the extension ran for more than {} ms without returning", budget.as_millis()))
            } else {
                Ok(UpdateDeadline::Continue(1))
            }
        });
        self.ticker.set(true);
        let r = f(&mut self.store, self.ext.as_ref());
        self.ticker.set(false);
        self.store.data_mut().flush_prints();
        match r {
            Ok(v) => match self.store.data().broken.clone() {
                Some(e) => Err(e),
                None => Ok(v),
            },
            Err(e) => {
                let frames = e
                    .downcast_ref::<WasmBacktrace>()
                    .map(|bt| {
                        bt.frames()
                            .iter()
                            .map(|f| {
                                let name = f.func_name().unwrap_or("<unknown>");
                                match f.symbols().first().and_then(|s| Some((s.file()?, s.line()?))) {
                                    Some((file, line)) => format!("{name} ({file}:{line})"),
                                    None => name.to_string(),
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // The trap, or the deepest cause (the CPU budget's own error sits under wasmtime's context).
                let message = match e.downcast_ref::<wasmtime::Trap>() {
                    Some(t) => t.to_string(),
                    None => e.root_cause().to_string(),
                };
                self.store.data_mut().send(&ToApp::Trap(Trap { message: message.clone(), frames }));
                Err(message)
            }
        }
    }

    fn complete(&mut self, ticket: u64, completion: crate::bindings::codeyard::api::types::Completion) -> Result<(), String> {
        self.call(limits::GUEST_CALL_MILLIS, |s, x| x.expect("instantiated").codeyard_api_guest().call_complete(s, ticket, &completion))
    }

    /// Deliver tickets that finished inside the exthost (refused calls). Delivering one may refuse another, so this
    /// loops, but a guest that keeps refusing itself is cut off.
    fn drain(&mut self) -> Result<(), String> {
        for _ in 0..1000 {
            let local = std::mem::take(&mut self.store.data_mut().local);
            if local.is_empty() {
                return Ok(());
            }
            for (ticket, c) in local {
                self.complete(ticket, c.into())?;
            }
        }
        Err("the extension kept making refused calls".into())
    }
}
