//! Atomic primitives for the concurrent collector.
//!
//! v0.2's [`Phase`](crate::Phase) and [`WriteBarrier`](crate::WriteBarrier)
//! were single-threaded scaffolding. v0.3 lifts them into the multi-threaded
//! world: an [`AtomicPhase`] readable from any mutator with `Acquire`
//! ordering, and an [`AtomicWriteBarrier`] that pushes overwritten
//! references into a shared [`RememberedSet`](crate::RememberedSet) instead
//! of a single-threaded `Vec`.
//!
//! Memory ordering choices (and why):
//!
//! - The collector publishes `Phase::Marking` with `Release` so that any
//!   subsequent write-barrier read sees a fully-published mark stack.
//! - Mutators read the phase with `Acquire`, pairing with that release.
//! - Phase transitions use compare-and-swap so the collector can detect a
//!   misordered call and refuse to advance.

use crate::remembered_set::RememberedSet;
use crate::Phase;
use std::sync::atomic::{AtomicU8, Ordering};

const PHASE_IDLE: u8 = 0;
const PHASE_MARKING: u8 = 1;
const PHASE_SWEEPING: u8 = 2;

pub struct AtomicPhase(AtomicU8);

impl AtomicPhase {
    pub fn new() -> Self {
        Self(AtomicU8::new(PHASE_IDLE))
    }

    pub fn load(&self) -> Phase {
        decode(self.0.load(Ordering::Acquire))
    }

    pub fn store(&self, p: Phase) {
        self.0.store(encode(p), Ordering::Release);
    }

    /// Atomic state transition. Returns `true` iff the previous phase
    /// matched `from`. The intended state machine is
    /// `Idle -> Marking -> Sweeping -> Idle`; out-of-order calls are
    /// refused at the CAS so the collector can detect bugs immediately.
    pub fn transition(&self, from: Phase, to: Phase) -> bool {
        self.0
            .compare_exchange(
                encode(from),
                encode(to),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

impl Default for AtomicPhase {
    fn default() -> Self {
        Self::new()
    }
}

fn encode(p: Phase) -> u8 {
    match p {
        Phase::Idle => PHASE_IDLE,
        Phase::Marking => PHASE_MARKING,
        Phase::Sweeping => PHASE_SWEEPING,
    }
}

fn decode(v: u8) -> Phase {
    match v {
        PHASE_IDLE => Phase::Idle,
        PHASE_MARKING => Phase::Marking,
        PHASE_SWEEPING => Phase::Sweeping,
        _ => unreachable!("invalid phase byte: {}", v),
    }
}

/// Thread-safe SATB write barrier. The mutator calls
/// [`pre_write`](Self::pre_write) before overwriting a reference; if the
/// collector is in [`Phase::Marking`], the previous target is recorded into
/// the shared [`RememberedSet`].
pub struct AtomicWriteBarrier<'a> {
    phase: &'a AtomicPhase,
    set: &'a RememberedSet,
}

impl<'a> AtomicWriteBarrier<'a> {
    pub fn new(phase: &'a AtomicPhase, set: &'a RememberedSet) -> Self {
        Self { phase, set }
    }

    pub fn pre_write(&self, overwritten: u32) {
        if self.phase.load() == Phase::Marking {
            self.set.record(overwritten);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn load_store_roundtrip() {
        let p = AtomicPhase::new();
        assert_eq!(p.load(), Phase::Idle);
        p.store(Phase::Marking);
        assert_eq!(p.load(), Phase::Marking);
        p.store(Phase::Sweeping);
        assert_eq!(p.load(), Phase::Sweeping);
    }

    #[test]
    fn transition_succeeds_only_on_matching_from() {
        let p = AtomicPhase::new();
        assert!(p.transition(Phase::Idle, Phase::Marking));
        assert_eq!(p.load(), Phase::Marking);
        // Wrong from value should refuse.
        assert!(!p.transition(Phase::Idle, Phase::Sweeping));
        assert_eq!(p.load(), Phase::Marking);
        assert!(p.transition(Phase::Marking, Phase::Sweeping));
        assert!(p.transition(Phase::Sweeping, Phase::Idle));
        assert_eq!(p.load(), Phase::Idle);
    }

    #[test]
    fn idle_barrier_is_no_op_under_atomics() {
        let phase = AtomicPhase::new();
        let set = RememberedSet::new();
        let barrier = AtomicWriteBarrier::new(&phase, &set);
        barrier.pre_write(42);
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn marking_barrier_records_atomically() {
        let phase = AtomicPhase::new();
        phase.store(Phase::Marking);
        let set = RememberedSet::new();
        let barrier = AtomicWriteBarrier::new(&phase, &set);
        barrier.pre_write(7);
        barrier.pre_write(11);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn many_writers_one_collector() {
        // Spawn 8 mutator threads, each performing 100 pre_writes during
        // the marking phase. The remembered set should collect every write.
        let phase = Arc::new(AtomicPhase::new());
        let set = Arc::new(RememberedSet::new());
        phase.store(Phase::Marking);

        let n_threads = 8;
        let writes_per = 100;
        let mut handles = vec![];
        for thread_id in 0..n_threads {
            let phase = Arc::clone(&phase);
            let set = Arc::clone(&set);
            handles.push(thread::spawn(move || {
                let barrier = AtomicWriteBarrier::new(&phase, &set);
                for i in 0..writes_per {
                    barrier.pre_write(thread_id * 1000 + i);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(set.len(), n_threads as usize * writes_per as usize);
    }

    #[test]
    fn writers_under_idle_phase_record_nothing() {
        let phase = Arc::new(AtomicPhase::new()); // starts Idle
        let set = Arc::new(RememberedSet::new());

        let mut handles = vec![];
        for _ in 0..4 {
            let phase = Arc::clone(&phase);
            let set = Arc::clone(&set);
            handles.push(thread::spawn(move || {
                let barrier = AtomicWriteBarrier::new(&phase, &set);
                for i in 0..100 {
                    barrier.pre_write(i);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(set.len(), 0, "Idle phase should record nothing");
    }
}
