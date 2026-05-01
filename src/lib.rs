//! samsara - endless rebirth. Allocate, mark, sweep, repeat.
//!
//! A research-grade garbage collector built up in deliberate layers:
//!
//! 1. Bump-allocating arena (v0.0)
//! 2. Tri-color marker over an abstract object graph (v0.0)
//! 3. Mark-region heap with per-region states + sweep (v0.1)
//! 4. Snapshot-at-the-beginning (SATB) write barrier
//! 5. Lock-free remembered sets, generational regions
//! 6. Concurrent marking (mutator + collector running simultaneously)
//!
//! v0.1 ships [`RegionHeap`] - fixed-size regions with state machines and a
//! sweep-and-reclaim cycle. The single-buffer [`Arena`] from v0.0 stays as
//! the simplest substrate; [`RegionHeap`] is what every later GC technique
//! is built on.

pub mod arena;
pub mod marker;
pub mod region;

pub use arena::{Arena, Handle};
pub use marker::{mark, Color, ObjectGraph};
pub use region::{Region, RegionHandle, RegionHeap, RegionState, REGION_SIZE};
