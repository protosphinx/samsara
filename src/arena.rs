//! Bump-allocating arena.
//!
//! v0 is single-threaded and untyped: hand out 32-bit byte offsets into a
//! pre-sized backing buffer. Allocation is `O(1)` (advance the bump pointer
//! with alignment); deallocation is the entire arena at once.
//!
//! v0.1 introduces per-thread arenas + atomic bump for lock-free concurrent
//! allocation. v0.2 introduces region-based partial reset.

const ALIGN: usize = 8;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Handle(pub u32);

pub struct Arena {
    bytes: Vec<u8>,
    offset: usize,
}

impl Arena {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            bytes: vec![0; cap],
            offset: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.bytes.len()
    }

    pub fn used(&self) -> usize {
        self.offset
    }

    /// Allocate `size` bytes, aligned to 8. Returns `None` on OOM.
    pub fn alloc(&mut self, size: usize) -> Option<Handle> {
        let aligned = (self.offset + (ALIGN - 1)) & !(ALIGN - 1);
        let end = aligned.checked_add(size)?;
        if end > self.bytes.len() {
            return None;
        }
        self.offset = end;
        Some(Handle(aligned as u32))
    }

    pub fn write(&mut self, h: Handle, data: &[u8]) {
        let start = h.0 as usize;
        self.bytes[start..start + data.len()].copy_from_slice(data);
    }

    pub fn read(&self, h: Handle, len: usize) -> &[u8] {
        let start = h.0 as usize;
        &self.bytes[start..start + len]
    }

    /// Reset the bump pointer. All outstanding handles are invalidated; the
    /// caller is responsible for not using them. (A safe API lives at v0.2.)
    pub fn reset(&mut self) {
        self.offset = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_advances_offset_with_alignment() {
        let mut a = Arena::with_capacity(64);
        let h0 = a.alloc(3).unwrap();
        let h1 = a.alloc(5).unwrap();
        assert_eq!(h0.0, 0);
        assert_eq!(h1.0, 8); // aligned up from 3 → 8
    }

    #[test]
    fn alloc_returns_none_on_oom() {
        let mut a = Arena::with_capacity(16);
        assert!(a.alloc(8).is_some());
        assert!(a.alloc(8).is_some());
        assert!(a.alloc(1).is_none());
    }

    #[test]
    fn write_then_read_round_trips() {
        let mut a = Arena::with_capacity(64);
        let h = a.alloc(4).unwrap();
        a.write(h, &[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(a.read(h, 4), &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn reset_zeros_offset() {
        let mut a = Arena::with_capacity(64);
        a.alloc(16).unwrap();
        assert_eq!(a.used(), 16);
        a.reset();
        assert_eq!(a.used(), 0);
    }
}
