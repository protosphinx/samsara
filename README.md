<h1 align="center">samsara</h1>

<p align="center"><em>संसार - endless rebirth. Allocate, mark, sweep, repeat.</em></p>

---

A research-grade garbage collector in Rust. Mark-region heap, snapshot-at-the-beginning write barrier, lock-free remembered sets, concurrent marking - built up in deliberate layers from the bump allocator out.

## The bet

Modern GC research is dominated by JVM and CLR work that assumes a managed runtime. The interesting question is what a tracing collector looks like *as a Rust crate consumed by ordinary programs* - pluggable into your own object graph, abstract over how you store nodes, with the tri-color machinery exposed and testable in isolation.

samsara starts there: an abstract `ObjectGraph` trait, a stop-the-world tri-color marker that consumes it, a bump arena underneath. Each subsequent version adds one piece of the modern GC stack - barrier, regions, generations, concurrency.

## Roadmap

| v   | Layer | Status |
|-----|-------|--------|
| 0.0 | Bump arena + tri-color marker over `ObjectGraph` | **shipped** |
| 0.1 | Mark-region heap with per-region states + sweep cycle | **shipped** |
| 0.2 | Snapshot-at-the-beginning write barrier (Yuasa, 1990) | **shipped** |
| 0.3 | Atomic phase + thread-safe write barrier + multi-producer remembered set | **shipped** |
| 0.4 | Concurrent marker thread; mutator and collector run simultaneously | **shipped** |
| 0.5 | Treiber lock-free stack as a faster RememberedSet substrate | **shipped** |
| 0.6 | Hazard-pointer registry primitive (acquire/protect/clear/scan) | **shipped** |
| 0.7 | SafeTreiberStack with hazard-protected pop + deferred reclamation (ABA-safe) | **shipped** |
| 0.8 | Loom-checked correctness under all interleavings | next |
| 0.9 | Generational regions | |
| 0.5 | Loom-checked correctness + replay-tested mutator/collector races | |

## The tri-color invariant

The mark phase maintains: *no black object references a white object*. The marker enforces this trivially in v0 (stop-the-world). In v0.2+ the write barrier preserves it under concurrent mutation - every reference write to a white target either greys the target or greys the source (Dijkstra-style insertion barrier vs. SATB deletion barrier).

samsara picks SATB (Yuasa, 1990) because its overhead is bounded by allocation rate rather than write rate, and because the analysis of remembered-set correctness is cleaner.

## Use

```rust
use samsara::{mark, Arena, Color, ObjectGraph};

struct MyHeap { /* ... */ }
impl ObjectGraph for MyHeap {
    type Id = u32;
    fn roots(&self) -> Vec<u32> { /* ... */ }
    fn refs_of(&self, id: u32) -> Vec<u32> { /* ... */ }
    fn all_ids(&self) -> Vec<u32> { /* ... */ }
}

let coloring = mark(&heap);
let unreachable: Vec<_> = coloring
    .iter()
    .filter(|(_, c)| **c == Color::White)
    .map(|(id, _)| *id)
    .collect();
```

## Reading

- Dijkstra, Lamport, Martin, Scholten, Steffens - *On-the-Fly Garbage Collection: an Exercise in Cooperation* (1978). The original tri-color analysis.
- Yuasa - *Real-time garbage collection on general-purpose machines* (1990). SATB.
- Blackburn, McKinley - *Immix: a mark-region garbage collector with space efficiency, fast collection, and mutator performance* (2008). The region-based design samsara converges on.

## License

MIT.

---

<p align="center"><a href="https://x.com/protosphinx">@protosphinx</a></p>
