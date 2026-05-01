# GOALS - samsara

Sequenced milestones to a working concurrent mark-region GC.

## v0.0 - substrate ✦ **shipped**

- Bump arena: alignment-aware `alloc`, untyped `read`/`write`, full reset
- Tri-color marker over `ObjectGraph` trait
- Tests: reachability, tri-color invariant, cycle termination, OOM

## v0.1 - mark-region heap ✦ **shipped**

- 32 KiB regions; `Free` / `Allocating` / `Full` state machine
- `RegionHeap`: alloc with current-region bumping + spillover, write/read,
  `pre_mark` / `mark_live` / `sweep` cycle
- Tests: spillover when full, sweep reclaims dead regions, partial-live
  regions are not compacted, full alloc/mark/sweep cycle continues allocating

## v0.2 - write barrier ✦ **shipped**

- `MarkStack` and `Phase` (Idle / Marking / Sweeping)
- `WriteBarrier` with `pre_write` capturing overwritten references during Marking
- Tests: barrier no-ops in Idle and Sweeping; captures in Marking; positive
  demonstration that SATB preserves objects orphaned mid-marking; negative
  control showing the same scenario loses objects without the barrier

## v0.3 - atomic phase + remembered set ✦ **shipped**

- `AtomicPhase` with Acquire/Release semantics and CAS-based transitions
- `RememberedSet` (multi-producer Mutex<Vec> queue) for capture
- `AtomicWriteBarrier` over `(AtomicPhase, RememberedSet)`
- Tests: 16 threads × 200 records all arrive; Idle phase records nothing;
  CAS refuses out-of-order transitions

## v0.4 - concurrent marking

- Mutator and collector run simultaneously
- Handshake protocol for stack snapshotting
- Loom-checked: tri-color invariant under N concurrent mutators × 1 collector

## v0.4 - generational + remembered sets

- Young / old region partition
- Lock-free card-marking remembered set
- Promotion policy

## v0.5 - formal validation

- Loom: full mark phase under all schedule interleavings up to 4 mutators
- Recorded mutator traces: replay-test the collector against frozen workloads
- Mutator/collector races: shown safe by construction

## Non-goals

- Stack-walking / reflection-based root scanning. Roots come from the user.
- Drop-in replacement for `Box`/`Rc`. samsara is a substrate, not a sugar.
- Compaction beyond Immix-style opportunistic evacuation.
