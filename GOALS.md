# GOALS — samsara

Sequenced milestones to a working concurrent mark-region GC.

## v0.0 — substrate ✦ **shipped**

- Bump arena: alignment-aware `alloc`, untyped `read`/`write`, full reset
- Tri-color marker over `ObjectGraph` trait
- Tests: reachability, tri-color invariant, cycle termination, OOM

## v0.1 — mark-region heap

- Heap split into 32 KiB regions (Immix-shape)
- Per-region free lists
- Region states: free / partially-allocated / full / collecting
- Defragmentation: opportunistic evacuation from sparse regions

## v0.2 — write barrier

- Snapshot-at-the-beginning (Yuasa) deletion barrier
- Per-thread mark-stack
- Barrier overhead microbenchmarks

## v0.3 — concurrent marking

- Mutator and collector run simultaneously
- Handshake protocol for stack snapshotting
- Loom-checked: tri-color invariant under N concurrent mutators × 1 collector

## v0.4 — generational + remembered sets

- Young / old region partition
- Lock-free card-marking remembered set
- Promotion policy

## v0.5 — formal validation

- Loom: full mark phase under all schedule interleavings up to 4 mutators
- Recorded mutator traces: replay-test the collector against frozen workloads
- Mutator/collector races: shown safe by construction

## Non-goals

- Stack-walking / reflection-based root scanning. Roots come from the user.
- Drop-in replacement for `Box`/`Rc`. samsara is a substrate, not a sugar.
- Compaction beyond Immix-style opportunistic evacuation.
