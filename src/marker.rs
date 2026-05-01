//! Tri-color abstract marker (Dijkstra et al., 1978).
//!
//! The garbage-collection literature is built on three colors:
//!
//! - **White**: unreached so far. Provisionally garbage.
//! - **Grey**: reached, but its outgoing references not yet scanned.
//! - **Black**: reached and fully scanned.
//!
//! Mark phase invariant (the *tri-color invariant*): no black object holds
//! a reference to a white object. When the grey set is empty, every
//! still-white object is unreachable from the roots and may be reclaimed.
//!
//! This module is generic over any [`ObjectGraph`]. v0 is stop-the-world; the
//! concurrent variant (v0.3) re-uses this marker behind a write barrier.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Color {
    White,
    Grey,
    Black,
}

/// An abstract object graph. The marker uses only [`roots`] (entry points)
/// and [`refs_of`] (outgoing edges per node) - no notion of allocation,
/// type, or layout leaks in.
///
/// [`roots`]: ObjectGraph::roots
/// [`refs_of`]: ObjectGraph::refs_of
pub trait ObjectGraph {
    type Id: Copy + Eq + Hash;

    fn roots(&self) -> Vec<Self::Id>;
    fn refs_of(&self, id: Self::Id) -> Vec<Self::Id>;
    fn all_ids(&self) -> Vec<Self::Id>;
}

/// Stop-the-world tri-color marking. Returns a coloring map where every
/// reachable object is `Black` and every unreachable object is `White`.
///
/// Termination: the grey set strictly shrinks each iteration that does not
/// discover a new node, and grows only by nodes never seen before - so the
/// algorithm terminates after at most `|V| + |E|` operations on a finite
/// graph.
pub fn mark<G: ObjectGraph>(graph: &G) -> HashMap<G::Id, Color> {
    let mut color: HashMap<G::Id, Color> = graph
        .all_ids()
        .into_iter()
        .map(|id| (id, Color::White))
        .collect();
    let mut grey: VecDeque<G::Id> = VecDeque::new();
    let mut seen: HashSet<G::Id> = HashSet::new();

    for r in graph.roots() {
        if seen.insert(r) {
            color.insert(r, Color::Grey);
            grey.push_back(r);
        }
    }

    while let Some(id) = grey.pop_front() {
        for child in graph.refs_of(id) {
            if seen.insert(child) {
                color.insert(child, Color::Grey);
                grey.push_back(child);
            }
        }
        color.insert(id, Color::Black);
    }

    color
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-drawn graph for tests.
    ///
    /// ```text
    ///   roots: {0, 1}
    ///   0 → 2 → 3
    ///   1 → 3
    ///   4 (orphan, unreachable)
    ///   5 → 6  (both unreachable)
    /// ```
    struct G;

    impl ObjectGraph for G {
        type Id = u32;
        fn roots(&self) -> Vec<u32> {
            vec![0, 1]
        }
        fn refs_of(&self, id: u32) -> Vec<u32> {
            match id {
                0 => vec![2],
                1 => vec![3],
                2 => vec![3],
                5 => vec![6],
                _ => vec![],
            }
        }
        fn all_ids(&self) -> Vec<u32> {
            (0..=6).collect()
        }
    }

    #[test]
    fn reachable_objects_end_up_black() {
        let c = mark(&G);
        for id in [0u32, 1, 2, 3] {
            assert_eq!(c[&id], Color::Black, "expected {} black", id);
        }
    }

    #[test]
    fn unreachable_objects_stay_white() {
        let c = mark(&G);
        for id in [4u32, 5, 6] {
            assert_eq!(c[&id], Color::White, "expected {} white", id);
        }
    }

    #[test]
    fn no_grey_objects_remain_after_marking() {
        let c = mark(&G);
        assert!(c.values().all(|color| *color != Color::Grey));
    }

    #[test]
    fn tri_color_invariant_holds() {
        // No black object references a white object.
        let c = mark(&G);
        for (&id, &color) in &c {
            if color == Color::Black {
                for child in G.refs_of(id) {
                    assert_ne!(
                        c[&child],
                        Color::White,
                        "black {} → white {} violates the tri-color invariant",
                        id,
                        child
                    );
                }
            }
        }
    }

    #[test]
    fn cycles_terminate() {
        struct Cycle;
        impl ObjectGraph for Cycle {
            type Id = u32;
            fn roots(&self) -> Vec<u32> {
                vec![0]
            }
            fn refs_of(&self, id: u32) -> Vec<u32> {
                match id {
                    0 => vec![1],
                    1 => vec![2],
                    2 => vec![0], // cycle back
                    _ => vec![],
                }
            }
            fn all_ids(&self) -> Vec<u32> {
                vec![0, 1, 2]
            }
        }
        let c = mark(&Cycle);
        assert!(c.values().all(|color| *color == Color::Black));
    }
}
