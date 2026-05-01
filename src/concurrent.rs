//! Concurrent marking - a worker thread drains the mark stack while the
//! mutator continues to issue write-barrier captures.
//!
//! The shape:
//!
//! ```text
//!   collector thread                       mutator thread
//!   ----------------                       ----------------
//!   while phase == Marking:                ... ordinary work ...
//!     drain remembered_set -> stack          pre_write(old_target):
//!     pop stack                                if phase == Marking:
//!       scan, recurse                            remembered_set.record
//!     if stack empty:
//!       sleep briefly
//!   final drain
//!   return black set
//! ```
//!
//! Termination protocol: the mutator (or a separate orchestrator) flips
//! the [`AtomicPhase`] from `Marking` to anything else when it has stopped
//! producing new captures. The collector then drains any remaining
//! remembered-set entries one last time and exits.
//!
//! Correctness sketch (informal): every object reachable from the roots at
//! the start of marking either
//!
//! 1. survives the marker's BFS traversal of the original graph, or
//! 2. is captured by the SATB barrier when its incoming reference is
//!    overwritten and lands in the remembered set.
//!
//! Both paths end at the marker's `black` set. Because we drain the
//! remembered set both inside the loop and after the phase flips, no
//! recorded id can be missed. v0.5 adds Loom-checked tests against this
//! claim under all interleavings.

use crate::{AtomicPhase, MarkStack, ObjectGraph, Phase, RememberedSet};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub fn concurrent_mark<G>(
    graph: Arc<G>,
    stack: Arc<Mutex<MarkStack>>,
    remembered: Arc<RememberedSet>,
    phase: Arc<AtomicPhase>,
) -> JoinHandle<HashSet<u32>>
where
    G: ObjectGraph<Id = u32> + Send + Sync + 'static,
{
    thread::spawn(move || {
        let mut black: HashSet<u32> = HashSet::new();
        loop {
            // Pull anything in the remembered set onto the local stack.
            let drained = remembered.drain();
            if !drained.is_empty() {
                let mut s = stack.lock().expect("mark stack poisoned");
                for id in drained {
                    s.push(id);
                }
            }

            let next = stack.lock().expect("mark stack poisoned").pop();
            match next {
                Some(id) => {
                    if !black.insert(id) {
                        continue;
                    }
                    for child in graph.refs_of(id) {
                        if !black.contains(&child) {
                            stack.lock().expect("mark stack poisoned").push(child);
                        }
                    }
                }
                None => {
                    // Nothing left to scan. Decide whether to terminate.
                    if phase.load() != Phase::Marking {
                        // Final-drain the remembered set under quiescence.
                        let final_drain = remembered.drain();
                        if final_drain.is_empty()
                            && stack.lock().expect("mark stack poisoned").is_empty()
                        {
                            return black;
                        }
                        let mut s = stack.lock().expect("mark stack poisoned");
                        for id in final_drain {
                            s.push(id);
                        }
                    } else {
                        // Phase still Marking - mutator may produce more captures.
                        thread::sleep(Duration::from_micros(50));
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AtomicWriteBarrier;
    use std::time::Instant;

    /// 1 -> 2 -> 3;  4 -> 5;  4 and 5 are unreachable from the roots.
    struct StaticGraph;

    impl ObjectGraph for StaticGraph {
        type Id = u32;
        fn roots(&self) -> Vec<u32> {
            vec![1]
        }
        fn refs_of(&self, id: u32) -> Vec<u32> {
            match id {
                1 => vec![2],
                2 => vec![3],
                4 => vec![5],
                _ => vec![],
            }
        }
        fn all_ids(&self) -> Vec<u32> {
            vec![1, 2, 3, 4, 5]
        }
    }

    fn fresh_state() -> (
        Arc<StaticGraph>,
        Arc<Mutex<MarkStack>>,
        Arc<RememberedSet>,
        Arc<AtomicPhase>,
    ) {
        let graph = Arc::new(StaticGraph);
        let stack = Arc::new(Mutex::new(MarkStack::new()));
        let remembered = Arc::new(RememberedSet::new());
        let phase = Arc::new(AtomicPhase::new());
        (graph, stack, remembered, phase)
    }

    #[test]
    fn concurrent_marker_drains_reachable_set() {
        let (graph, stack, remembered, phase) = fresh_state();

        // Push roots, transition to Marking, spawn the worker.
        phase.store(Phase::Marking);
        for r in graph.roots() {
            stack.lock().unwrap().push(r);
        }
        let handle = concurrent_mark(
            Arc::clone(&graph),
            Arc::clone(&stack),
            Arc::clone(&remembered),
            Arc::clone(&phase),
        );

        // Mutator does no captures; just signals end-of-marking.
        thread::sleep(Duration::from_millis(5));
        phase.store(Phase::Idle);

        let black = handle.join().unwrap();
        assert!(black.contains(&1));
        assert!(black.contains(&2));
        assert!(black.contains(&3));
        // 4 and 5 unreachable.
        assert!(!black.contains(&4));
        assert!(!black.contains(&5));
    }

    #[test]
    fn concurrent_marker_picks_up_satb_captures_during_marking() {
        let (graph, stack, remembered, phase) = fresh_state();
        phase.store(Phase::Marking);
        for r in graph.roots() {
            stack.lock().unwrap().push(r);
        }
        let handle = concurrent_mark(
            Arc::clone(&graph),
            Arc::clone(&stack),
            Arc::clone(&remembered),
            Arc::clone(&phase),
        );

        // Mutator: capture object 4 (as if its reference were being overwritten
        // somewhere). This pulls 4 into the live set, and 5 transitively.
        let barrier = AtomicWriteBarrier::new(&phase, &remembered);
        barrier.pre_write(4);

        // Give the worker time to drain the captured entry.
        thread::sleep(Duration::from_millis(20));
        phase.store(Phase::Idle);

        let black = handle.join().unwrap();
        assert!(black.contains(&1));
        assert!(black.contains(&2));
        assert!(black.contains(&3));
        assert!(black.contains(&4), "SATB-captured 4 must be marked");
        assert!(black.contains(&5), "5 must be reached transitively from 4");
    }

    #[test]
    fn concurrent_marker_terminates_quickly_with_no_work() {
        let (graph, stack, remembered, phase) = fresh_state();
        phase.store(Phase::Marking);
        // No roots pushed.
        let handle = concurrent_mark(
            Arc::clone(&graph),
            Arc::clone(&stack),
            Arc::clone(&remembered),
            Arc::clone(&phase),
        );
        phase.store(Phase::Idle);

        let started = Instant::now();
        let black = handle.join().unwrap();
        assert!(black.is_empty());
        assert!(started.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn concurrent_marker_handles_many_captures_from_busy_mutator() {
        let (graph, stack, remembered, phase) = fresh_state();
        phase.store(Phase::Marking);
        stack.lock().unwrap().push(1);
        let handle = concurrent_mark(
            Arc::clone(&graph),
            Arc::clone(&stack),
            Arc::clone(&remembered),
            Arc::clone(&phase),
        );

        // Mutator captures the same id many times in quick succession.
        let phase_clone = Arc::clone(&phase);
        let remembered_clone = Arc::clone(&remembered);
        let mutator = thread::spawn(move || {
            let barrier = AtomicWriteBarrier::new(&phase_clone, &remembered_clone);
            for _ in 0..1000 {
                barrier.pre_write(4);
            }
        });
        mutator.join().unwrap();
        phase.store(Phase::Idle);

        let black = handle.join().unwrap();
        // Even with thousands of duplicate captures, marking is correct: the
        // black set is reachable + captured, with no double-counting issues.
        assert!(black.contains(&4));
        assert!(black.contains(&5));
    }
}
