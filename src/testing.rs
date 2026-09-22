use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

use crate::{
    client::{Reply, Transport},
    clock::Clock,
    error::Error,
};

/// A clock that only moves when somebody sleeps.
pub struct FakeClock {
    start: Instant,
    offset: Cell<Duration>,
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            offset: Cell::new(Duration::ZERO),
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.offset.get()
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.start + self.offset.get()
    }

    fn sleep(&self, duration: Duration) {
        self.offset.set(self.offset.get() + duration);
    }
}

#[derive(Clone)]
pub enum Step {
    Answer(u16, String),
    Refused,
}

pub fn ok(body: &str) -> Step {
    Step::Answer(200, body.to_string())
}

/// Scripted answers per path. The last step repeats, so a test only scripts
/// what changes.
#[derive(Default)]
pub struct FakeTransport {
    gets: RefCell<HashMap<String, VecDeque<Step>>>,
    puts: RefCell<VecDeque<Step>>,
    deletes: RefCell<VecDeque<Step>>,
    pub written: RefCell<Vec<(String, String)>>,
    /// The paths `delete` was called with, in order.
    pub deleted: RefCell<Vec<String>>,
    /// Every request that is not a GET, method and path, in order. A test
    /// that must prove a removal happened *before* a write reads this one:
    /// `written` and `deleted` alone cannot say which came first.
    pub calls: RefCell<Vec<(&'static str, String)>>,
}

impl FakeTransport {
    pub fn on_get(self, path: &str, steps: Vec<Step>) -> Self {
        self.gets
            .borrow_mut()
            .insert(path.to_string(), steps.into());
        self
    }

    pub fn on_put(self, steps: Vec<Step>) -> Self {
        *self.puts.borrow_mut() = steps.into();
        self
    }

    /// The answers to `DELETE`, in their own queue: a test scripts removals
    /// and writes apart, since they are refused for different reasons.
    pub fn on_delete(self, steps: Vec<Step>) -> Self {
        *self.deletes.borrow_mut() = steps.into();
        self
    }

    fn record(&self, method: &'static str, path: &str) {
        self.calls.borrow_mut().push((method, path.to_string()));
    }
}

fn take(queue: Option<&mut VecDeque<Step>>) -> Option<Step> {
    let queue = queue?;
    if queue.len() > 1 {
        queue.pop_front()
    } else {
        queue.front().cloned()
    }
}

fn play(step: Option<Step>, method: &'static str, path: &str) -> Result<Reply, Error> {
    match step {
        Some(Step::Answer(status, body)) => Ok(Reply { status, body }),
        Some(Step::Refused) => Err(Error::Request {
            method,
            path: path.to_string(),
            reason: "connection refused".to_string(),
        }),
        None => panic!("test did not script {method} {path}"),
    }
}

impl Transport for FakeTransport {
    fn get(&self, path: &str) -> Result<Reply, Error> {
        let step = take(self.gets.borrow_mut().get_mut(path));
        play(step, "GET", path)
    }

    fn put_json(&self, path: &str, body: &str) -> Result<Reply, Error> {
        self.written
            .borrow_mut()
            .push((path.to_string(), body.to_string()));
        self.record("PUT", path);
        let step = take(Some(&mut *self.puts.borrow_mut()));
        play(step, "PUT", path)
    }

    /// Shares the write queue with `put_json`: a test scripts the answers to
    /// writes, whichever verb the service uses.
    fn post_json(&self, path: &str, body: &str) -> Result<Reply, Error> {
        self.written
            .borrow_mut()
            .push((path.to_string(), body.to_string()));
        self.record("POST", path);
        let step = take(Some(&mut *self.puts.borrow_mut()));
        play(step, "POST", path)
    }

    /// The same write queue again.
    fn patch_json(&self, path: &str, body: &str) -> Result<Reply, Error> {
        self.written
            .borrow_mut()
            .push((path.to_string(), body.to_string()));
        self.record("PATCH", path);
        let step = take(Some(&mut *self.puts.borrow_mut()));
        play(step, "PATCH", path)
    }

    /// Its own queue, and its own record: `written` stays what it was, so a
    /// test that counts writes is not confused by a removal.
    fn delete(&self, path: &str) -> Result<Reply, Error> {
        self.deleted.borrow_mut().push(path.to_string());
        self.record("DELETE", path);
        let step = take(Some(&mut *self.deletes.borrow_mut()));
        play(step, "DELETE", path)
    }
}
