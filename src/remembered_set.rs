//! A shared multi-producer remembered set.
//!
//! Used by [`AtomicWriteBarrier`](crate::AtomicWriteBarrier) to capture
//! references overwritten during marking. Many mutator threads can `record`
//! concurrently; the collector calls `drain` once at the end of marking to
//! pull every captured id onto its mark stack.
//!
//! v0.3 wraps a `Mutex<Vec<u32>>`. This is enough to demonstrate
//! correctness; v0.4 swaps the implementation for a per-thread bounded
//! buffer with a lock-free spillover, which is what production GCs use to
//! make `record` cheap on the hot path.

use std::sync::Mutex;

#[derive(Default)]
pub struct RememberedSet {
    inner: Mutex<Vec<u32>>,
}

impl RememberedSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `id` onto the shared queue. Multiple threads may call this
    /// concurrently; the lock serializes them.
    pub fn record(&self, id: u32) {
        self.inner.lock().expect("remembered set poisoned").push(id);
    }

    /// Empty the queue and return its contents. Intended for the collector
    /// to call once at the end of marking.
    pub fn drain(&self) -> Vec<u32> {
        std::mem::take(&mut *self.inner.lock().expect("remembered set poisoned"))
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("remembered set poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn record_then_drain() {
        let s = RememberedSet::new();
        s.record(1);
        s.record(2);
        s.record(3);
        let drained = s.drain();
        assert_eq!(drained, vec![1, 2, 3]);
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn drain_returns_then_clears() {
        let s = RememberedSet::new();
        s.record(42);
        assert_eq!(s.drain(), vec![42]);
        assert!(s.is_empty());
        // Subsequent records still work after drain.
        s.record(99);
        assert_eq!(s.drain(), vec![99]);
    }

    #[test]
    fn concurrent_records_all_arrive() {
        let s = Arc::new(RememberedSet::new());
        let n_threads = 16;
        let writes_per = 200;
        let mut handles = vec![];
        for t in 0..n_threads {
            let s = Arc::clone(&s);
            handles.push(thread::spawn(move || {
                for i in 0..writes_per {
                    s.record(t * 1000 + i);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let drained = s.drain();
        assert_eq!(drained.len(), n_threads as usize * writes_per as usize);
        // Every (t, i) pair shows up exactly once.
        let mut sorted = drained.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), drained.len(), "no duplicate records expected");
    }
}
