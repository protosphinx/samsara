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

## v0.4 - concurrent marking ✦ **shipped**

- `concurrent_mark` spawns a worker thread that drains the shared mark
  stack and remembered set while the mutator continues issuing captures
- Termination protocol: mutator flips `AtomicPhase` away from `Marking`;
  marker performs a final drain and exits
- Tests: reachable set marked, SATB captures picked up mid-run, busy
  mutator producing 1000 duplicate captures, prompt termination on no work

## v0.5 - Treiber lock-free stack ✦ **shipped**

- `TreiberStack` over `AtomicPtr<Node>` with push / pop / is_empty / Drop
- Compare-and-swap retry loop on uncontended path; no syscall ceiling
- Tests: single-thread LIFO order, 8-thread × 500-write multi-producer no
  loss, 4 producers + 4 consumers drain to empty, interleaved push/pop
  under contention with no duplicate values

## v0.6 - epoch-protected stack + Loom-checked correctness

- Hazard pointers or epoch-based reclamation to defeat ABA
- Wire the safe stack into `RememberedSet` behind a feature flag
- Loom tests for the marker under all interleavings up to N mutators

## v0.7 - generational + remembered sets

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
