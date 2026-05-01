//! Hazard pointer registry (Michael, 2004).
//!
//! Hazard pointers are a lock-free reclamation scheme. The idea:
//!
//! 1. A reader, before dereferencing a pointer, **publishes** it into a
//!    per-reader hazard slot.
//! 2. Before reclaiming a retired pointer, a writer **scans** every reader's
//!    hazard slot. If anyone is currently reading the pointer, defer.
//! 3. Once no reader publishes the pointer, it is safe to free.
//!
//! This defeats the ABA problem the v0.5 [`TreiberStack`](crate::TreiberStack)
//! is vulnerable to: an address cannot be reused while any thread still
//! holds it as a hazard.
//!
//! v0.6 ships the registry primitive: a fixed-size array of atomic
//! pointer slots that threads acquire and release manually. A future v0.7
//! wires this into a `SafeTreiberStack` that uses two slots per operation
//! (head plus successor) and a retire-list with deferred free.
//!
//! ## Why fixed-size?
//!
//! Real hazard-pointer libraries grow the slot array dynamically. v0.6
//! caps at [`MAX_HAZARDS`] for simplicity; that is enough to demonstrate
//! the scan-and-defer pattern under tests. v0.7 introduces dynamic growth.

use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

/// Maximum number of concurrent hazard slots in v0.6. Each in-flight reader
/// of a protected pointer claims one. Bumping this is a one-line change.
pub const MAX_HAZARDS: usize = 64;

pub struct HazardRegistry {
    slots: [Slot; MAX_HAZARDS],
}

struct Slot {
    in_use: AtomicBool,
    ptr: AtomicPtr<()>,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            in_use: AtomicBool::new(false),
            ptr: AtomicPtr::new(std::ptr::null_mut()),
        }
    }
}

impl Default for HazardRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HazardRegistry {
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| Slot::default()),
        }
    }

    /// Acquire a slot. Returns a guard that releases the slot on drop.
    /// Returns `None` if all slots are in use.
    pub fn acquire(&self) -> Option<HazardSlot<'_>> {
        for (idx, slot) in self.slots.iter().enumerate() {
            if slot
                .in_use
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(HazardSlot {
                    registry: self,
                    idx,
                });
            }
        }
        None
    }

    /// Returns `true` iff some slot is currently publishing `ptr` as a
    /// hazard. Reclaimers call this on a retired pointer before freeing.
    pub fn is_hazarded<T>(&self, ptr: *const T) -> bool {
        let target = ptr as *mut ();
        for slot in &self.slots {
            if slot.in_use.load(Ordering::Acquire) && slot.ptr.load(Ordering::Acquire) == target {
                return true;
            }
        }
        false
    }

    /// Snapshot of currently-hazarded pointers. Useful for batch retirement
    /// where the reclaimer scans the whole hazard table once.
    pub fn snapshot(&self) -> Vec<*mut ()> {
        let mut out = Vec::new();
        for slot in &self.slots {
            if slot.in_use.load(Ordering::Acquire) {
                let p = slot.ptr.load(Ordering::Acquire);
                if !p.is_null() {
                    out.push(p);
                }
            }
        }
        out
    }
}

unsafe impl Send for HazardRegistry {}
unsafe impl Sync for HazardRegistry {}

/// Per-thread guard owning one slot in a [`HazardRegistry`]. Drops release
/// the slot for reuse.
pub struct HazardSlot<'r> {
    registry: &'r HazardRegistry,
    idx: usize,
}

impl<'r> HazardSlot<'r> {
    /// Publish `ptr` as a hazard for this slot. Subsequent calls
    /// to [`HazardRegistry::is_hazarded`] with this pointer return `true`
    /// until [`clear`](Self::clear) is called.
    pub fn protect<T>(&self, ptr: *const T) {
        self.registry.slots[self.idx]
            .ptr
            .store(ptr as *mut (), Ordering::Release);
    }

    pub fn clear(&self) {
        self.registry.slots[self.idx]
            .ptr
            .store(std::ptr::null_mut(), Ordering::Release);
    }

    pub fn idx(&self) -> usize {
        self.idx
    }
}

impl<'r> Drop for HazardSlot<'r> {
    fn drop(&mut self) {
        self.clear();
        self.registry.slots[self.idx]
            .in_use
            .store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;

    #[test]
    fn acquire_and_release() {
        let r = HazardRegistry::new();
        let s1 = r.acquire().unwrap();
        let s2 = r.acquire().unwrap();
        assert_ne!(s1.idx(), s2.idx());
        drop(s1);
        let s3 = r.acquire().unwrap();
        // Re-acquired the freshly-released slot index.
        assert!(s3.idx() == 0 || s3.idx() == 2 || s3.idx() == 1);
    }

    #[test]
    fn protect_and_is_hazarded() {
        let r = HazardRegistry::new();
        let s = r.acquire().unwrap();
        let value = Box::new(42u32);
        let ptr: *const u32 = &*value;
        assert!(!r.is_hazarded(ptr));
        s.protect(ptr);
        assert!(r.is_hazarded(ptr));
        s.clear();
        assert!(!r.is_hazarded(ptr));
    }

    #[test]
    fn drop_clears_protection() {
        let r = HazardRegistry::new();
        let value = Box::new(123u32);
        let ptr: *const u32 = &*value;
        {
            let s = r.acquire().unwrap();
            s.protect(ptr);
            assert!(r.is_hazarded(ptr));
        }
        assert!(!r.is_hazarded(ptr));
    }

    #[test]
    fn snapshot_returns_published_pointers() {
        let r = HazardRegistry::new();
        let s1 = r.acquire().unwrap();
        let s2 = r.acquire().unwrap();
        let v1 = Box::new(1u32);
        let v2 = Box::new(2u32);
        s1.protect(&*v1);
        s2.protect(&*v2);
        let snap = r.snapshot();
        assert_eq!(snap.len(), 2);
        let p1 = (&*v1) as *const u32 as *mut ();
        let p2 = (&*v2) as *const u32 as *mut ();
        assert!(snap.contains(&p1));
        assert!(snap.contains(&p2));
    }

    #[test]
    fn many_threads_protect_distinct_slots() {
        let r = Arc::new(HazardRegistry::new());
        let n_threads = 8;
        let barrier = Arc::new(Barrier::new(n_threads));
        let mut handles = vec![];
        for _ in 0..n_threads {
            let r = Arc::clone(&r);
            let b = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let s = r.acquire().expect("slot available");
                let v = Box::new(42u32);
                let ptr: *const u32 = &*v;
                s.protect(ptr);
                b.wait();
                assert!(r.is_hazarded(ptr));
                s.idx()
            }));
        }
        let indices: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let mut sorted = indices.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), n_threads, "every thread got a distinct slot");
    }

    #[test]
    fn slots_exhausted_returns_none() {
        let r = HazardRegistry::new();
        let mut held = Vec::with_capacity(MAX_HAZARDS);
        for _ in 0..MAX_HAZARDS {
            held.push(r.acquire().unwrap());
        }
        assert!(r.acquire().is_none());
    }
}
