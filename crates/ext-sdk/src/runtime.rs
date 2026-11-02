//! A small single-threaded executor driven by the guest's exports. A host call returns a ticket; awaiting it parks the
//! task until `complete` delivers the result (and `event` its streamed data), which wakes the task. Every export
//! returns as soon as no task can make progress, so the extension never blocks the exthost.
//!
//! Everything lives in thread-locals: a WebAssembly instance has one thread, and each native test runs on its own.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use crate::protocol::{Completion, StreamEvent};
use crate::sys;
use crate::types::Ticket;

type Task = Pin<Box<dyn Future<Output = ()>>>;
type Ready = Arc<Mutex<VecDeque<u64>>>;

#[derive(Default)]
struct Slot {
    done: Option<Completion>,
    events: VecDeque<StreamEvent>,
    waker: Option<Waker>,
}

#[derive(Default)]
struct Runtime {
    /// `None` while the task is being polled.
    tasks: HashMap<u64, Option<Task>>,
    next_task: u64,
    tickets: HashMap<Ticket, Slot>,
    /// Request id → the task that answers it, so `cancel` can drop the task.
    requests: HashMap<u64, u64>,
}

thread_local! {
    static RT: RefCell<Runtime> = RefCell::new(Runtime::default());
    static READY: Ready = Arc::new(Mutex::new(VecDeque::new()));
}

struct TaskWaker {
    id: u64,
    ready: Ready,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let mut q = self.ready.lock().unwrap_or_else(|p| p.into_inner());
        if !q.contains(&self.id) {
            q.push_back(self.id);
        }
    }
}

fn ready() -> Ready {
    READY.with(Arc::clone)
}

/// Forget every task and ticket (a new mock host in a native test).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn reset() {
    let old = RT.with(|rt| std::mem::take(&mut *rt.borrow_mut()));
    ready().lock().unwrap_or_else(|p| p.into_inner()).clear();
    // Dropping tasks may cancel tickets, which calls the host: outside the borrow.
    drop(old);
}

/// Start a task; it first runs when the current export finishes its own work (`run`).
pub(crate) fn spawn(fut: impl Future<Output = ()> + 'static) -> u64 {
    let id = RT.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.next_task += 1;
        let id = rt.next_task;
        rt.tasks.insert(id, Some(Box::pin(fut)));
        id
    });
    ready().lock().unwrap_or_else(|p| p.into_inner()).push_back(id);
    id
}

/// Spawn the task that answers request `id`.
pub(crate) fn spawn_request(id: u64, fut: impl Future<Output = ()> + 'static) {
    let task = spawn(async move {
        fut.await;
        RT.with(|rt| rt.borrow_mut().requests.remove(&id));
    });
    RT.with(|rt| rt.borrow_mut().requests.insert(id, task));
}

/// The host stopped waiting for request `id`: drop its task, which cancels the tickets it waits on.
pub(crate) fn cancel_request(id: u64) {
    let task = RT.with(|rt| {
        let mut rt = rt.borrow_mut();
        let t = rt.requests.remove(&id)?;
        rt.tasks.remove(&t)
    });
    drop(task);
}

/// Poll every task that was woken until none can make progress.
pub(crate) fn run() {
    loop {
        let next = ready().lock().unwrap_or_else(|p| p.into_inner()).pop_front();
        let Some(id) = next else { return };
        let Some(mut task) = RT.with(|rt| rt.borrow_mut().tasks.get_mut(&id).and_then(Option::take)) else { continue };
        let waker = Waker::from(Arc::new(TaskWaker { id, ready: ready() }));
        let done = task.as_mut().poll(&mut Context::from_waker(&waker)).is_ready();
        let finished = RT.with(|rt| {
            let mut rt = rt.borrow_mut();
            match rt.tasks.get_mut(&id) {
                Some(slot) if !done => {
                    *slot = Some(task);
                    None
                }
                _ => {
                    rt.tasks.remove(&id);
                    Some(task)
                }
            }
        });
        drop(finished);
    }
}

/// `guest.complete`: wake whoever waits on `ticket`. Unknown tickets (cancelled, or never awaited) are dropped.
pub(crate) fn complete(ticket: Ticket, c: Completion) {
    let waker = RT.with(|rt| {
        let mut rt = rt.borrow_mut();
        let slot = rt.tickets.get_mut(&ticket)?;
        slot.done = Some(c);
        slot.waker.take()
    });
    if let Some(w) = waker {
        w.wake();
    }
}

/// `guest.event`: queue streamed data for `ticket`.
pub(crate) fn event(ticket: Ticket, e: StreamEvent) {
    let waker = RT.with(|rt| {
        let mut rt = rt.borrow_mut();
        let slot = rt.tickets.get_mut(&ticket)?;
        slot.events.push_back(e);
        slot.waker.take()
    });
    if let Some(w) = waker {
        w.wake();
    }
}

/// A ticket being waited on. Dropping it before it completes cancels the call (`tickets.cancel`).
pub(crate) struct Pending {
    ticket: Ticket,
    finished: bool,
}

impl Pending {
    pub(crate) fn new(ticket: Ticket) -> Pending {
        RT.with(|rt| rt.borrow_mut().tickets.insert(ticket, Slot::default()));
        Pending { ticket, finished: false }
    }

    /// The next streamed event, or `None` once the ticket has completed and every event was taken.
    pub(crate) fn next_event(&mut self) -> impl Future<Output = Option<StreamEvent>> + '_ {
        std::future::poll_fn(move |cx| {
            RT.with(|rt| {
                let mut rt = rt.borrow_mut();
                let Some(slot) = rt.tickets.get_mut(&self.ticket) else { return Poll::Ready(None) };
                if let Some(e) = slot.events.pop_front() {
                    return Poll::Ready(Some(e));
                }
                if slot.done.is_some() {
                    return Poll::Ready(None);
                }
                slot.waker = Some(cx.waker().clone());
                Poll::Pending
            })
        })
    }
}

impl Future for Pending {
    type Output = Completion;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Completion> {
        let ticket = self.ticket;
        let done = RT.with(|rt| {
            let mut rt = rt.borrow_mut();
            let slot = rt.tickets.get_mut(&ticket)?;
            match slot.done.take() {
                Some(c) => {
                    rt.tickets.remove(&ticket);
                    Some(c)
                }
                None => {
                    slot.waker = Some(cx.waker().clone());
                    None
                }
            }
        });
        match done {
            Some(c) => {
                self.finished = true;
                Poll::Ready(c)
            }
            // Gone without a completion: only after a reset.
            None if !RT.with(|rt| rt.borrow().tickets.contains_key(&ticket)) => {
                self.finished = true;
                Poll::Ready(Completion::Failed(crate::Error::Cancelled))
            }
            None => Poll::Pending,
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let waiting = RT.with(|rt| rt.borrow_mut().tickets.remove(&self.ticket)).is_some_and(|s| s.done.is_none());
        if waiting {
            sys::cancel_ticket(self.ticket);
        }
    }
}
