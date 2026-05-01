//! ABA-safe Treiber stack via hazard pointers + deferred reclamation.
//!
//! Three differences from the v0.5 [`TreiberStack`](crate::TreiberStack):
//!
//! 1. **Pop protects the head pointer** in a [`HazardSlot`](crate::HazardSlot)
//!    before dereferencing. Other threads cannot reclaim the node while
//!    the slot publishes it as a hazard.
//! 2. **Successful pops retire** the popped node into a per-stack queue
//!    rather than freeing it immediately. The retire path walks the
//!    queue and frees only entries no thread is currently hazarding.
//! 3. **Verify after protect.** Between loading `head` and publishing
//!    the hazard, the head can change. The pop loop reloads after
//!    `protect` and retries on mismatch - this is the load-bearing
//!    correctness step.
//!
//! Tradeoffs: pop is now slower (one slot acquisition + one extra atomic
//! load per attempt) but ABA-free. Retire scans cost `O(retired ·
//! hazards_in_use)` and are amortized over `RETIRE_THRESHOLD` retires.

use crate::hazard::HazardRegistry;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Mutex;

struct Node {
    value: u32,
    next: *mut Node,
}

/// Reclaim retired nodes after the queue grows past this many entries.
const RETIRE_THRESHOLD: usize = 64;

pub struct SafeTreiberStack {
    head: AtomicPtr<Node>,
    hazards: HazardRegistry,
    retired: Mutex<Vec<*mut Node>>,
}

unsafe impl Send for SafeTreiberStack {}
unsafe impl Sync for SafeTreiberStack {}

impl Default for SafeTreiberStack {
    fn default() -> Self {
        Self::new()
    }
}

impl SafeTreiberStack {
    pub fn new() -> Self {
        Self {
            head: AtomicPtr::new(ptr::null_mut()),
            hazards: HazardRegistry::new(),
            retired: Mutex::new(Vec::new()),
        }
    }

    pub fn push(&self, value: u32) {
        let new = Box::into_raw(Box::new(Node {
            value,
            next: ptr::null_mut(),
        }));
        loop {
            let head = self.head.load(Ordering::Acquire);
            // SAFETY: `new` is a fresh allocation only this thread can see
            // until the CAS succeeds.
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
        let slot = self.hazards.acquire().expect("hazard slot exhausted");
        loop {
            let head = self.head.load(Ordering::Acquire);
            if head.is_null() {
                return None;
            }
            slot.protect(head);
            // The verify step: ensure head is still the head AFTER the
            // hazard is published. If not, retry. This prevents a popper
            // from being interrupted between load and protect, with the
            // node freed in the gap.
            if self.head.load(Ordering::Acquire) != head {
                continue;
            }
            // SAFETY: head is hazarded and is still the head per the
            // recheck above; no thread can reclaim it before we either
            // succeed or release the slot.
            let next = unsafe { (*head).next };
            if self
                .head
                .compare_exchange(head, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                // SAFETY: we won the CAS so we own the popped node logically.
                // Read its value before retiring; other threads might still
                // hazard the pointer briefly, but they will reload, see it
                // is no longer the head, and retry.
                let value = unsafe { (*head).value };
                slot.clear();
                drop(slot);
                self.retire(head);
                return Some(value);
            }
            // CAS failed - someone else won. Loop, re-protect.
        }
    }

    fn retire(&self, ptr: *mut Node) {
        let mut retired = self.retired.lock().expect("retired mutex poisoned");
        retired.push(ptr);
        if retired.len() >= RETIRE_THRESHOLD {
            self.reclaim_locked(&mut retired);
        }
    }

    fn reclaim_locked(&self, retired: &mut Vec<*mut Node>) {
        let hazards = self.hazards.snapshot();
        retired.retain(|&ptr| {
            let p = ptr as *mut ();
            if hazards.contains(&p) {
                true
            } else {
                // SAFETY: no thread hazards this pointer; the only owner is
                // the retired queue, which is releasing it now.
                unsafe { drop(Box::from_raw(ptr)) };
                false
            }
        });
    }

    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire).is_null()
    }

    /// Force a reclamation pass. Useful in tests to make leak checks
    /// deterministic without waiting for the threshold.
    pub fn force_reclaim(&self) {
        let mut retired = self.retired.lock().expect("retired mutex poisoned");
        self.reclaim_locked(&mut retired);
    }
}

impl Drop for SafeTreiberStack {
    fn drop(&mut self) {
        while self.pop().is_some() {}
        let mut retired = self.retired.lock().expect("retired mutex poisoned");
        // No threads can be reading any more (we have &mut self), so free
        // everything outright.
        for ptr in retired.drain(..) {
            // SAFETY: see Drop comment above.
            unsafe { drop(Box::from_raw(ptr)) };
        }
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
        let s = SafeTreiberStack::new();
        s.push(1);
        s.push(2);
        s.push(3);
        assert_eq!(s.pop(), Some(3));
        assert_eq!(s.pop(), Some(2));
        assert_eq!(s.pop(), Some(1));
        assert_eq!(s.pop(), None);
    }

    #[test]
    fn drop_drains_retired_queue() {
        let s = SafeTreiberStack::new();
        for i in 0..200 {
            s.push(i);
        }
        for _ in 0..200 {
            s.pop();
        }
        // Drop runs and must not leak; verified under Miri/ASan when
        // available, plus the explicit force_reclaim path below.
        s.force_reclaim();
    }

    #[test]
    fn many_writers_one_reader_no_loss() {
        let s = Arc::new(SafeTreiberStack::new());
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
            assert!(seen.insert(v), "duplicate {}", v);
        }
        assert_eq!(seen.len(), n_threads as usize * writes_per as usize);
    }

    #[test]
    fn concurrent_push_and_pop_drain_to_empty() {
        let s = Arc::new(SafeTreiberStack::new());
        let n_threads = 4;
        let writes_per = 500;
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
    fn interleaved_high_contention_no_duplicates() {
        // The same scenario the v0.5 toy stack flakes on under ABA.
        // The safe stack must give zero duplicates at any contention level.
        let s = Arc::new(SafeTreiberStack::new());
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
        while let Some(v) = s.pop() {
            assert!(seen.insert(v));
        }
        assert_eq!(seen.len(), (n_threads * ops_per) as usize);
    }

    #[test]
    fn force_reclaim_frees_unhazarded() {
        let s = SafeTreiberStack::new();
        for i in 0..32 {
            s.push(i);
        }
        for _ in 0..32 {
            s.pop();
        }
        // Push enough retires to make reclaim do work.
        s.force_reclaim();
        // The retired queue should be empty afterwards (no concurrent hazards).
        let retired = s.retired.lock().unwrap();
        assert_eq!(retired.len(), 0);
    }
}
