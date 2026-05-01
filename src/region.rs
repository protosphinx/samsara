//! Mark-region heap (Blackburn–McKinley 2008, Immix-shape).
//!
//! The heap is an array of fixed-size **regions**. Allocation walks regions
//! looking for one with space; freed regions go back to the pool. Each
//! region carries:
//!
//! - a state machine (`Free`, `Allocating`, `Full`)
//! - a bump pointer for in-region allocation
//! - a live-object count, recomputed each mark phase
//!
//! After a [`mark`](crate::mark) phase, the user calls
//! [`RegionHeap::pre_mark`] / [`RegionHeap::mark_live`] / [`RegionHeap::sweep`]
//! to drive the collector cycle:
//!
//! ```text
//! pre_mark   →   reset live counts to 0
//! mark_live  →   for each reachable handle, bump its region's live count
//! sweep      →   regions with live_count == 0 → reset to Free
//! ```
//!
//! Regions with at least one live object stay in place — the whole point of
//! mark-*region* (versus mark-*sweep*) is that you don't need to rescan
//! object headers; you just decide per-region whether to keep it.
//!
//! v0.1 is single-threaded. v0.2 adds the SATB write barrier; v0.3 makes the
//! mutator and collector concurrent.

const ALIGN: usize = 8;

/// Region size: 32 KiB. Same scale as Immix's chunk size; small enough that
/// dead regions are reclaimed promptly, large enough that bump-allocation
/// dominates over region-search overhead.
pub const REGION_SIZE: usize = 32 * 1024;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RegionState {
    /// No outstanding allocations; bump pointer at zero. Eligible for reuse.
    Free,
    /// Currently being allocated into. The heap's `current` pointer references
    /// at most one such region.
    Allocating,
    /// Bump pointer hit the end. No more allocations until sweep.
    Full,
}

pub struct Region {
    bytes: Box<[u8; REGION_SIZE]>,
    offset: usize,
    state: RegionState,
    /// Set to zero by [`RegionHeap::pre_mark`]; bumped by
    /// [`RegionHeap::mark_live`]; consulted by [`RegionHeap::sweep`].
    live_count: u32,
}

impl Region {
    fn new() -> Self {
        Self {
            bytes: Box::new([0; REGION_SIZE]),
            offset: 0,
            state: RegionState::Free,
            live_count: 0,
        }
    }

    pub fn state(&self) -> RegionState {
        self.state
    }

    pub fn used(&self) -> usize {
        self.offset
    }

    pub fn live_count(&self) -> u32 {
        self.live_count
    }

    fn try_alloc(&mut self, size: usize) -> Option<usize> {
        let aligned = (self.offset + (ALIGN - 1)) & !(ALIGN - 1);
        let end = aligned.checked_add(size)?;
        if end > REGION_SIZE {
            self.state = RegionState::Full;
            return None;
        }
        self.offset = end;
        self.state = RegionState::Allocating;
        Some(aligned)
    }

    fn reset(&mut self) {
        self.offset = 0;
        self.state = RegionState::Free;
        self.live_count = 0;
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RegionHandle {
    pub region: u32,
    pub offset: u32,
}

pub struct RegionHeap {
    regions: Vec<Region>,
    current: Option<usize>,
}

impl RegionHeap {
    /// Construct a heap with `n` empty regions. Total capacity is
    /// `n × REGION_SIZE` bytes.
    pub fn with_regions(n: usize) -> Self {
        Self {
            regions: (0..n).map(|_| Region::new()).collect(),
            current: None,
        }
    }

    pub fn n_regions(&self) -> usize {
        self.regions.len()
    }

    pub fn n_free(&self) -> usize {
        self.regions
            .iter()
            .filter(|r| r.state == RegionState::Free)
            .count()
    }

    pub fn region(&self, idx: usize) -> &Region {
        &self.regions[idx]
    }

    /// Allocate `size` bytes. Returns `None` if no region has room *and* no
    /// region is free. `size` larger than [`REGION_SIZE`] always returns
    /// `None` (object-too-large is a v0.2 problem).
    pub fn alloc(&mut self, size: usize) -> Option<RegionHandle> {
        if size > REGION_SIZE {
            return None;
        }
        if let Some(idx) = self.current {
            if let Some(off) = self.regions[idx].try_alloc(size) {
                return Some(RegionHandle {
                    region: idx as u32,
                    offset: off as u32,
                });
            }
        }
        // Current region full or unset — find a free region.
        for (i, r) in self.regions.iter_mut().enumerate() {
            if r.state == RegionState::Free {
                if let Some(off) = r.try_alloc(size) {
                    self.current = Some(i);
                    return Some(RegionHandle {
                        region: i as u32,
                        offset: off as u32,
                    });
                }
            }
        }
        None
    }

    pub fn read(&self, h: RegionHandle, len: usize) -> &[u8] {
        let r = &self.regions[h.region as usize];
        let start = h.offset as usize;
        &r.bytes[start..start + len]
    }

    pub fn write(&mut self, h: RegionHandle, data: &[u8]) {
        let r = &mut self.regions[h.region as usize];
        let start = h.offset as usize;
        r.bytes[start..start + data.len()].copy_from_slice(data);
    }

    /// Reset all per-region live counts before a mark phase.
    pub fn pre_mark(&mut self) {
        for r in &mut self.regions {
            r.live_count = 0;
        }
    }

    /// Record that `h` survived the mark phase. Bumps its region's live count.
    pub fn mark_live(&mut self, h: RegionHandle) {
        self.regions[h.region as usize].live_count += 1;
    }

    /// Reclaim every region whose live count is zero. Returns the number of
    /// regions freed. Partially-live regions are untouched (no compaction).
    pub fn sweep(&mut self) -> usize {
        let mut freed = 0;
        for (i, r) in self.regions.iter_mut().enumerate() {
            if r.state != RegionState::Free && r.live_count == 0 {
                r.reset();
                if self.current == Some(i) {
                    self.current = None;
                }
                freed += 1;
            }
        }
        freed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_heap_has_all_regions_free() {
        let h = RegionHeap::with_regions(4);
        assert_eq!(h.n_regions(), 4);
        assert_eq!(h.n_free(), 4);
    }

    #[test]
    fn alloc_lands_in_first_free_region() {
        let mut h = RegionHeap::with_regions(4);
        let a = h.alloc(64).unwrap();
        assert_eq!(a.region, 0);
        assert_eq!(a.offset, 0);
        let b = h.alloc(64).unwrap();
        assert_eq!(b.region, 0);
        assert_eq!(b.offset, 64);
    }

    #[test]
    fn alloc_spills_when_region_fills() {
        let mut h = RegionHeap::with_regions(2);
        // Fill region 0 completely.
        let mut total = 0;
        while let Some(handle) = h.alloc(1024) {
            assert_eq!(handle.region, 0);
            total += 1024;
            if total >= REGION_SIZE {
                break;
            }
        }
        // Next allocation lands in region 1.
        let next = h.alloc(64).unwrap();
        assert_eq!(next.region, 1);
    }

    #[test]
    fn alloc_returns_none_when_heap_is_full() {
        let mut h = RegionHeap::with_regions(1);
        // Allocate the entire region.
        let big = h.alloc(REGION_SIZE).unwrap();
        let _ = big;
        // Now there's nothing left.
        assert!(h.alloc(8).is_none());
    }

    #[test]
    fn write_then_read_round_trips() {
        let mut h = RegionHeap::with_regions(1);
        let a = h.alloc(8).unwrap();
        h.write(a, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(h.read(a, 8), &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn sweep_frees_regions_with_no_live_objects() {
        let mut h = RegionHeap::with_regions(3);
        // Allocate one object in region 0.
        let a = h.alloc(REGION_SIZE - 8).unwrap(); // fills region 0
        let b = h.alloc(64).unwrap(); // forces spill; lands in region 1
        let c = h.alloc(64).unwrap(); // also region 1

        h.pre_mark();
        // Pretend only `c` survived.
        h.mark_live(c);
        let _ = (a, b);

        let freed = h.sweep();
        // region 0 had no marked objects → freed.
        // region 1 had `c` marked → kept.
        // region 2 was untouched → still Free, never re-counted.
        assert_eq!(freed, 1);
        assert_eq!(h.region(0).state(), RegionState::Free);
        assert_ne!(h.region(1).state(), RegionState::Free);
    }

    #[test]
    fn partially_live_region_is_not_compacted() {
        // The defining property of mark-region: live regions stay in place.
        let mut h = RegionHeap::with_regions(2);
        let a = h.alloc(128).unwrap();
        let _b = h.alloc(128).unwrap();
        let _c = h.alloc(128).unwrap();
        let used_before = h.region(a.region as usize).used();

        h.pre_mark();
        h.mark_live(a); // only `a` survives — but region stays
        h.sweep();

        let used_after = h.region(a.region as usize).used();
        assert_eq!(used_before, used_after, "partial region should not compact");
        assert_ne!(h.region(a.region as usize).state(), RegionState::Free);
    }

    #[test]
    fn alloc_larger_than_region_returns_none() {
        let mut h = RegionHeap::with_regions(4);
        assert!(h.alloc(REGION_SIZE + 1).is_none());
    }

    #[test]
    fn full_cycle_reclaims_dead_regions_and_continues_allocating() {
        let mut h = RegionHeap::with_regions(2);
        let a = h.alloc(REGION_SIZE - 8).unwrap();
        let _b = h.alloc(64).unwrap();
        let _c = h.alloc(64).unwrap();

        h.pre_mark();
        h.mark_live(a);
        let freed = h.sweep();
        assert!(freed >= 1);

        // After sweep, allocator should successfully use a freed region.
        let d = h.alloc(64).unwrap();
        // d landed in some region that was previously dead.
        assert_ne!(d.region, a.region);
    }
}
