use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct TrackingAllocator;

static TOTAL_ALLOCATED: AtomicU64 = AtomicU64::new(0);
static TOTAL_DEALLOCATED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Default)]
pub struct AllocationSnapshot {
    pub allocated: u64,
    pub deallocated: u64,
}

#[derive(Clone, Copy, Default)]
pub struct AllocationDelta {
    pub allocated_bytes: u64,
    pub deallocated_bytes: u64,
}

impl AllocationDelta {
    pub fn net_bytes(&self) -> i64 {
        self.allocated_bytes as i64 - self.deallocated_bytes as i64
    }
}

impl AllocationSnapshot {
    pub fn delta_since(&self, previous: AllocationSnapshot) -> AllocationDelta {
        AllocationDelta {
            allocated_bytes: self.allocated.saturating_sub(previous.allocated),
            deallocated_bytes: self.deallocated.saturating_sub(previous.deallocated),
        }
    }
}

pub fn allocation_snapshot() -> AllocationSnapshot {
    AllocationSnapshot {
        allocated: TOTAL_ALLOCATED.load(Ordering::Relaxed),
        deallocated: TOTAL_DEALLOCATED.load(Ordering::Relaxed),
    }
}

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            TOTAL_ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            TOTAL_ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        TOTAL_DEALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = System.realloc(ptr, layout, new_size);
        if !new_ptr.is_null() {
            TOTAL_ALLOCATED.fetch_add(new_size as u64, Ordering::Relaxed);
            TOTAL_DEALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        new_ptr
    }
}
