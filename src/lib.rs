//! samsara - endless rebirth. Allocate, mark, sweep, repeat.
//!
//! A research-grade garbage collector built up in deliberate layers:
//!
//! 1. Bump-allocating arena (v0.0)
//! 2. Tri-color marker over an abstract object graph (v0.0)
//! 3. Mark-region heap with per-region states + sweep (v0.1)
//! 4. Snapshot-at-the-beginning (SATB) write barrier (v0.2)
//! 5. Lock-free remembered sets, generational regions
//! 6. Concurrent marking (mutator + collector running simultaneously)
//!
//! v0.2 ships the [`barrier`] module: a `MarkStack`, a `Phase` state machine,
//! and the SATB `WriteBarrier`. Tests demonstrate the property the barrier
//! exists to provide: any object reachable from the roots at the start of
//! marking remains marked at the end, even if the mutator overwrites the
//! reference that connected it.

pub mod arena;
pub mod atomic;
pub mod barrier;
pub mod concurrent;
pub mod hazard;
pub mod marker;
pub mod region;
pub mod remembered_set;
pub mod treiber;

pub use arena::{Arena, Handle};
pub use atomic::{AtomicPhase, AtomicWriteBarrier};
pub use barrier::{MarkStack, Phase, WriteBarrier};
pub use concurrent::concurrent_mark;
pub use hazard::{HazardRegistry, HazardSlot};
pub use marker::{mark, Color, ObjectGraph};
pub use region::{Region, RegionHandle, RegionHeap, RegionState, REGION_SIZE};
pub use remembered_set::RememberedSet;
pub use treiber::TreiberStack;
