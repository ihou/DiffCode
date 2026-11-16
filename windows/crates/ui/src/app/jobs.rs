//! Work off the main thread. A job runs on a thread of its own; its result comes back to the main thread, where
//! the closure given with it applies it to the app at the start of the next frame.

use std::any::Any;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};

use crate::platform::Waker;

type Done<A> = Box<dyn FnOnce(&mut A, Box<dyn Any + Send>)>;
type Message<A> = Box<dyn FnOnce(&mut A) + Send>;

/// Hands work to the main thread from any thread (a file watcher, a terminal's reader).
pub struct Poster<A> {
    tx: Sender<Message<A>>,
    waker: Option<Waker>,
}

impl<A> Clone for Poster<A> {
    fn clone(&self) -> Self {
        Poster { tx: self.tx.clone(), waker: self.waker.clone() }
    }
}

impl<A> Poster<A> {
    /// Runs `f` with the model on the main thread, at the start of the next frame.
    pub fn post(&self, f: impl FnOnce(&mut A) + Send + 'static) {
        if self.tx.send(Box::new(f)).is_ok() {
            if let Some(w) = &self.waker {
                w();
            }
        }
    }
}

pub struct Jobs<A> {
    tx: Sender<(u64, Box<dyn Any + Send>)>,
    rx: Receiver<(u64, Box<dyn Any + Send>)>,
    pending: HashMap<u64, Done<A>>,
    next: u64,
    waker: Option<Waker>,
    messages_tx: Sender<Message<A>>,
    messages_rx: Receiver<Message<A>>,
}

impl<A> Default for Jobs<A> {
    fn default() -> Self {
        let (tx, rx) = channel();
        let (messages_tx, messages_rx) = channel();
        Jobs { tx, rx, pending: HashMap::new(), next: 1, waker: None, messages_tx, messages_rx }
    }
}

impl<A: 'static> Jobs<A> {
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    pub fn waker(&self) -> Option<Waker> {
        self.waker.clone()
    }

    /// Runs `work` on another thread, then `done` with its result on the main thread. Returns the job's number.
    pub fn spawn<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut A, T) + 'static,
    ) -> u64 {
        let id = self.next;
        self.next += 1;
        self.pending.insert(id, Box::new(move |app, any| {
            if let Ok(t) = any.downcast::<T>() {
                done(app, *t)
            }
        }));
        let tx = self.tx.clone();
        let waker = self.waker.clone();
        std::thread::Builder::new()
            .name("diffcode-job".into())
            .spawn(move || {
                let result = work();
                let _ = tx.send((id, Box::new(result)));
                if let Some(w) = waker {
                    w();
                }
            })
            .expect("thread");
        id
    }

    /// Whether any job is still running.
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The finished jobs' results, with what to do with each. The caller runs them with the app.
    pub fn finished(&mut self) -> Vec<(Done<A>, Box<dyn Any + Send>)> {
        let mut out = Vec::new();
        while let Ok((id, result)) = self.rx.try_recv() {
            if let Some(done) = self.pending.remove(&id) {
                out.push((done, result));
            }
        }
        out
    }

    /// Something other threads can hand work back through without a job.
    pub fn poster(&self) -> Poster<A> {
        Poster { tx: self.messages_tx.clone(), waker: self.waker.clone() }
    }

    /// Runs everything that finished or was posted, with the model it belongs to.
    pub fn run(target: &mut A, jobs: impl Fn(&mut A) -> &mut Jobs<A>) {
        let finished = jobs(target).finished();
        let mut messages = Vec::new();
        while let Ok(m) = jobs(target).messages_rx.try_recv() {
            messages.push(m);
        }
        for (done, result) in finished {
            done(target, result);
        }
        for m in messages {
            m(target);
        }
    }
}
