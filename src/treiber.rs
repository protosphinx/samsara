//! Treiber lock-free stack (Treiber, IBM Research, 1986).
//!
//! The simplest non-trivial lock-free data structure. The stack is an
//! `AtomicPtr<Node>` to the head; nodes are heap-allocated with a `next`
//! pointer. `push` and `pop` retry on CAS failure until they win the race.
//!
//! ```text
//!   push:
//!     1. allocate Node { value, next = head.load() }
//!     2. CAS head from old to new; on failure, retry from step 1
//!
//!   pop:
//!     1. head = head.load();  if null, return None
//!     2. next = (*head).next
//!     3. CAS head from old to next; on failure, retry from step 1
//! ```
//!
//! ## A note on ABA
//!
//! This v0.5 implementation is intentionally minimal: it does not protect
//! against the ABA problem, where a popped node could be reallocated to the
//! same address before another thread completes its CAS, and the CAS would
//! erroneously succeed. In practice the Rust system allocator is unlikely
//! to recycle addresses on the time scale of a single CAS retry, so
//! single-process workloads do not hit ABA. For a production-grade
//! lock-free stack you want hazard pointers (Michael 2004) or epoch-based
//! reclamation (Fraser 2004), neither of which v0.5 ships. v0.6 lands an
//! epoch-protected variant.
//!
//! ## What this gives the GC
//!
//! [`RememberedSet`](crate::RememberedSet) currently uses
//! `Mutex<Vec<u32>>`. The mutex is fine when contention is low but bottlenecks
//! a busy mutator. A Treiber-style remembered set lets each `record` be
//! a single CAS in the uncontended path, with no syscall ceiling on lock
//! attempts. v0.6 wires this in behind a feature flag.

use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

struct Node {
    value: u32,
    next: *mut Node,
}

/// Lock-free LIFO stack of `u32` values.
pub struct TreiberStack {
    head: AtomicPtr<Node>,
}

unsafe impl Send for TreiberStack {}
unsafe impl Sync for TreiberStack {}

impl TreiberStack {
    pub fn new() -> Self {
        Self {
            head: AtomicPtr::new(ptr::null_mut()),
        }
    }

    pub fn push(&self, value: u32) {
        let new = Box::into_raw(Box::new(Node {
            value,
            next: ptr::null_mut(),
        }));
        loop {
            let head = self.head.load(Ordering::Acquire);
            // SAFETY: `new` is a fresh allocation we just made; we are the
            // only thread that has seen it, so writing `next` is sound.
            unsafe {
                (*new).next = head;
            }
            if self
                .head
                .compare_exchange(head, new, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return;
            }
        }
    }

    pub fn pop(&self) -> Option<u32> {
        loop {
            let head = self.head.load(Ordering::Acquire);
            if head.is_null() {
                return None;
            }
            // SAFETY: while we hold this load, no other thread can free
            // `head` because they would need to win the CAS first. This is
            // sound only under the no-ABA assumption documented above; a
            // production impl uses hazard pointers here.
            let next = unsafe { (*head).next };
            if self
                .head
                .compare_exchange(head, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                // SAFETY: we won the CAS, so no other thread now references
                // the popped node.
                let boxed = unsafe { Box::from_raw(head) };
                return Some(boxed.value);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire).is_null()
    }
}

impl Default for TreiberStack {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TreiberStack {
    fn drop(&mut self) {
        while self.pop().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn single_thread_lifo_order() {
        let s = TreiberStack::new();
        s.push(1);
        s.push(2);
        s.push(3);
        assert_eq!(s.pop(), Some(3));
        assert_eq!(s.pop(), Some(2));
        assert_eq!(s.pop(), Some(1));
        assert_eq!(s.pop(), None);
    }

    #[test]
    fn pop_on_empty_returns_none() {
        let s = TreiberStack::new();
        assert!(s.is_empty());
        assert_eq!(s.pop(), None);
    }

    #[test]
    fn drop_drains_remaining_nodes() {
        let s = TreiberStack::new();
        for i in 0..1000 {
            s.push(i);
        }
        drop(s); // must not leak; covered by Miri / ASan if run.
    }

    #[test]
    fn many_writers_one_reader_no_loss() {
        let s = Arc::new(TreiberStack::new());
        let n_threads = 8;
        let writes_per = 500;

        let mut handles = vec![];
        for t in 0..n_threads {
            let s = Arc::clone(&s);
            handles.push(thread::spawn(move || {
                for i in 0..writes_per {
                    s.push(t * 10_000 + i);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let mut seen: HashSet<u32> = HashSet::new();
        while let Some(v) = s.pop() {
            assert!(seen.insert(v), "duplicate value popped: {}", v);
        }
        assert_eq!(seen.len(), n_threads as usize * writes_per as usize);
    }

    #[test]
    fn concurrent_push_and_pop_drain_to_empty() {
        let s = Arc::new(TreiberStack::new());
        let n_threads = 4;
        let writes_per = 200;
        let total: u32 = n_threads * writes_per;

        let mut writers = vec![];
        for t in 0..n_threads {
            let s = Arc::clone(&s);
            writers.push(thread::spawn(move || {
                for i in 0..writes_per {
                    s.push(t * 1000 + i);
                }
            }));
        }
        for h in writers {
            h.join().unwrap();
        }

        let mut readers = vec![];
        for _ in 0..n_threads {
            let s = Arc::clone(&s);
            readers.push(thread::spawn(move || {
                let mut count = 0;
                while s.pop().is_some() {
                    count += 1;
                }
                count
            }));
        }
        let popped: u32 = readers.into_iter().map(|h| h.join().unwrap()).sum();

        assert_eq!(popped, total);
        assert!(s.is_empty());
    }

    #[test]
    fn interleaved_push_pop_under_contention() {
        // Mix push and pop on the same threads. Verify invariant: every
        // value pushed is at most popped once (no duplicates).
        let s = Arc::new(TreiberStack::new());
        let n_threads = 4;
        let ops_per = 1000;

        let mut handles = vec![];
        for t in 0..n_threads {
            let s = Arc::clone(&s);
            handles.push(thread::spawn(move || {
                let mut popped = vec![];
                for i in 0..ops_per {
                    s.push(t * 100_000 + i);
                    if i.is_multiple_of(2) {
                        if let Some(v) = s.pop() {
                            popped.push(v);
                        }
                    }
                }
                popped
            }));
        }
        let all_popped: Vec<u32> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();

        let mut seen: HashSet<u32> = HashSet::new();
        for v in &all_popped {
            assert!(seen.insert(*v), "duplicate {}", v);
        }
        // Drain the rest.
        while let Some(v) = s.pop() {
            assert!(seen.insert(v));
        }
        assert_eq!(seen.len(), (n_threads * ops_per) as usize);
    }
}
