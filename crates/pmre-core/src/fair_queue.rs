//! FairQueue — a thread-safe, closable FIFO work queue.
//!
//! Each `pop()` call blocks until an item is available or the queue is sealed+empty.
//! Items are delivered in FIFO order. Once `seal()` is called, no more items may be
//! pushed; consumers drain the remaining items and then receive `None`.
//!
//! Use-case: distribute kernel work items (pixel rows, tiles) fairly across a pool
//! of worker threads so that no worker starves while others finish early.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

struct Inner<T> {
    items: VecDeque<T>,
    sealed: bool,
}

/// A cloneable handle to a shared FIFO work queue.
/// All clones share the same underlying queue.
pub struct FairQueue<T: Send + 'static> {
    state: Arc<(Mutex<Inner<T>>, Condvar)>,
}

impl<T: Send + 'static> FairQueue<T> {
    pub fn new() -> Self {
        Self {
            state: Arc::new((
                Mutex::new(Inner {
                    items: VecDeque::new(),
                    sealed: false,
                }),
                Condvar::new(),
            )),
        }
    }

    /// Push a work item. Panics if called after `seal()`.
    pub fn push(&self, item: T) {
        let (lock, cvar) = &*self.state;
        let mut g = lock.lock().unwrap();
        debug_assert!(!g.sealed, "push after seal");
        g.items.push_back(item);
        cvar.notify_one();
    }

    /// Seal the queue — no more items will be pushed.
    /// Wakes all blocked `pop()` callers so they can drain and exit.
    pub fn seal(&self) {
        let (lock, cvar) = &*self.state;
        lock.lock().unwrap().sealed = true;
        cvar.notify_all();
    }

    /// Pull the next item, blocking if the queue is currently empty but not sealed.
    /// Returns `None` when the queue is both sealed and empty.
    pub fn pop(&self) -> Option<T> {
        let (lock, cvar) = &*self.state;
        let mut g = lock.lock().unwrap();
        loop {
            if let Some(item) = g.items.pop_front() {
                return Some(item);
            }
            if g.sealed {
                return None;
            }
            g = cvar.wait(g).unwrap();
        }
    }
}

impl<T: Send + 'static> Clone for FairQueue<T> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl<T: Send + 'static> Default for FairQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}
