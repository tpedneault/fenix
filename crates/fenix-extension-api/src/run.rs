//! Work that waits, written as `async`: an HTTP request, another
//! extension's service, a timer. [`spawn`] starts it, and it runs on as
//! the answers come -- which they do as events, so the extension's
//! [`crate::Extension::handle`] gives each to [`deliver`] first.
//!
//! ```no_run
//! use fenix_extension_api::{run, Event, Extension, HttpRequest};
//!
//! #[derive(Default)]
//! struct Weather;
//!
//! impl Extension for Weather {
//!     fn run_command(&mut self, _id: &str) -> Result<(), String> {
//!         run::spawn(async {
//!             match run::fetch(HttpRequest::get("https://api.example.com/now")).await {
//!                 Ok(r) => fenix_extension_api::notify(&r.text()),
//!                 Err(e) => fenix_extension_api::notify(&e),
//!             }
//!         });
//!         Ok(())
//!     }
//!
//!     fn handle(&mut self, event: Event) {
//!         let Some(_event) = run::deliver(event) else { return };
//!         // ... what isn't an answer a job was waiting for
//!     }
//! }
//! ```
//!
//! One thread, no waker: whenever an answer comes, every job still
//! waiting is polled again.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use crate::{host, Event, HttpRequest, HttpResponse};

type Job = Pin<Box<dyn Future<Output = ()>>>;

/// What came back for a request.
enum Answer {
    Http(Result<HttpResponse, String>),
    Service(Result<String, String>),
    Timer,
}

/// A request's place: still waiting, or answered.
enum Slot {
    Waiting,
    Came(Answer),
}

#[derive(Default)]
struct Executor {
    jobs: Vec<Job>,
    /// Started while the others were being polled.
    started: Vec<Job>,
    polling: bool,
    slots: HashMap<u64, Slot>,
}

thread_local! {
    static EXECUTOR: RefCell<Executor> = RefCell::new(Executor::default());
}

fn waker() -> Waker {
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    fn noop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    // SAFETY: every function in the table ignores its (null) data.
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}

/// Starts `job`: it runs as far as it can now, and on as answers come.
pub fn spawn(job: impl Future<Output = ()> + 'static) {
    let polling = EXECUTOR.with(|e| {
        let mut e = e.borrow_mut();
        e.started.push(Box::pin(job));
        e.polling
    });
    // A job starting another: that one runs when this round is done.
    if !polling {
        poll_all();
    }
}

/// How many jobs are still waiting.
pub fn waiting() -> usize {
    EXECUTOR.with(|e| e.borrow().jobs.len())
}

fn poll_all() {
    let waker = waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        let mut jobs = EXECUTOR.with(|e| {
            let mut e = e.borrow_mut();
            e.polling = true;
            let mut jobs = std::mem::take(&mut e.jobs);
            jobs.append(&mut e.started);
            jobs
        });
        jobs.retain_mut(|job| job.as_mut().poll(&mut cx).is_pending());
        let again = EXECUTOR.with(|e| {
            let mut e = e.borrow_mut();
            e.polling = false;
            e.jobs.append(&mut jobs);
            !e.started.is_empty()
        });
        if !again {
            break;
        }
    }
}

/// `event`, unless it's an answer a job was waiting for: then that job
/// runs on, and `None` comes back.
pub fn deliver(event: Event) -> Option<Event> {
    let id = match &event {
        Event::HttpResponse((id, _)) | Event::ServiceReply((id, _)) | Event::Timer(id) => *id,
        _ => return Some(event),
    };
    let waited = EXECUTOR.with(|e| matches!(e.borrow().slots.get(&id), Some(Slot::Waiting)));
    if !waited {
        return Some(event);
    }
    let answer = match event {
        Event::HttpResponse((_, r)) => Answer::Http(r),
        Event::ServiceReply((_, r)) => Answer::Service(r),
        _ => Answer::Timer,
    };
    EXECUTOR.with(|e| e.borrow_mut().slots.insert(id, Slot::Came(answer)));
    poll_all();
    None
}

/// Sends a request the first time it's polled, then waits for its answer.
struct Wait {
    id: Option<u64>,
    send: Option<Box<dyn FnOnce() -> Result<u64, String>>>,
}

impl Wait {
    fn new(send: impl FnOnce() -> Result<u64, String> + 'static) -> Wait {
        Wait { id: None, send: Some(Box::new(send)) }
    }
}

impl Future for Wait {
    type Output = Result<Answer, String>;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(send) = self.send.take() {
            let id = match send() {
                Ok(id) => id,
                Err(e) => return Poll::Ready(Err(e)),
            };
            EXECUTOR.with(|e| e.borrow_mut().slots.insert(id, Slot::Waiting));
            self.id = Some(id);
            return Poll::Pending;
        }
        let Some(id) = self.id else { return Poll::Pending };
        let came = EXECUTOR.with(|e| {
            let mut e = e.borrow_mut();
            match e.slots.remove(&id) {
                Some(Slot::Came(answer)) => Some(answer),
                Some(Slot::Waiting) => {
                    e.slots.insert(id, Slot::Waiting);
                    None
                }
                None => None,
            }
        });
        match came {
            Some(answer) => Poll::Ready(Ok(answer)),
            None => Poll::Pending,
        }
    }
}

/// An HTTP request, through Fenix's client: its response, whatever its
/// status.
pub async fn fetch(req: HttpRequest) -> Result<HttpResponse, String> {
    match Wait::new(move || host::http(&req)).await? {
        Answer::Http(r) => r,
        _ => Err("not an HTTP response".into()),
    }
}

/// Another extension's, or Fenix's, service `name`.
pub async fn call(name: &str, payload: &str) -> Result<String, String> {
    let (name, payload) = (name.to_string(), payload.to_string());
    match Wait::new(move || Ok(host::call_service(&name, &payload))).await? {
        Answer::Service(r) => r,
        _ => Err("not a service's answer".into()),
    }
}

/// Waits `millis`.
pub async fn sleep(millis: u32) {
    let _ = Wait::new(move || Ok(host::set_timer(millis, false))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slot answered by hand, as `deliver` fills one.
    fn answer(id: u64, text: &str) {
        EXECUTOR.with(|e| e.borrow_mut().slots.insert(id, Slot::Came(Answer::Service(Ok(text.into())))));
        poll_all();
    }

    #[test]
    fn a_job_runs_on_as_its_answers_come() {
        let got = std::rc::Rc::new(RefCell::new(Vec::new()));
        let seen = got.clone();
        let ids = std::rc::Rc::new(RefCell::new(vec![7u64, 8]));
        let next = move || -> Result<u64, String> { Ok(ids.borrow_mut().remove(0)) };
        let next2 = next.clone();
        spawn(async move {
            for send in [Box::new(next) as Box<dyn FnOnce() -> Result<u64, String>>, Box::new(next2)] {
                if let Ok(Answer::Service(Ok(text))) = (Wait { id: None, send: Some(send) }).await {
                    seen.borrow_mut().push(text);
                }
            }
        });
        assert_eq!(waiting(), 1);
        // Not ours: given back.
        assert!(deliver(Event::ServiceReply((99, Ok("x".into())))).is_some());
        answer(7, "first");
        assert_eq!(*got.borrow(), ["first"]);
        answer(8, "second");
        assert_eq!(*got.borrow(), ["first", "second"]);
        assert_eq!(waiting(), 0);
    }
}
