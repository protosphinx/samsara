//! samsara — endless rebirth. Allocate, mark, sweep, repeat.
//!
//! A research-grade garbage collector built up in deliberate layers:
//!
//! 1. Bump-allocating arena (this crate, v0)
//! 2. Tri-color marker over an abstract object graph (this crate, v0)
//! 3. Mark-region heap with per-region free lists
//! 4. Snapshot-at-the-beginning (SATB) write barrier
//! 5. Lock-free remembered sets, generational regions
//! 6. Concurrent marking (mutator + collector running simultaneously)
//!
//! v0 ships the substrate: a bump arena and a stop-the-world tri-color
//! marker that operates on any type implementing [`ObjectGraph`].
//! Concurrency primitives land in v0.3.

pub mod arena;
pub mod marker;

pub use arena::{Arena, Handle};
pub use marker::{mark, Color, ObjectGraph};
