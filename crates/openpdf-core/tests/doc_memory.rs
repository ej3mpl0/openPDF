//! `Document::memory_usage` must be cheap: no significant allocation.
//! Own test binary; the counter is thread-local, so allocations by other test
//! threads or the harness never count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;
thread_local! {
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
}

fn allocated() -> usize {
    ALLOCATED.with(Cell::get)
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCATED.try_with(|c| c.set(c.get() + l.size()));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn memory_usage_does_not_allocate_and_scales_with_streams() {
    // ~4 MiB of stream content over 400 pages.
    let doc = openpdf_core::blank_document(400).unwrap();
    let mut inner = lopdf::Document::load_mem(&doc.to_bytes().unwrap()).unwrap();
    for i in 0..400u32 {
        let content = vec![b'%'; 10_000 + i as usize];
        inner.add_object(lopdf::Stream::new(lopdf::dictionary! {}, content));
    }
    let mut bytes = Vec::new();
    inner.save_to(&mut bytes).unwrap();
    let len = bytes.len() as u64;
    let doc = openpdf_core::Document::from_bytes(bytes, None).unwrap();

    let before = allocated();
    let m = doc.memory_usage();
    let allocated = allocated() - before;
    eprintln!("large: file={len} {m:?} allocated_during_call={allocated}");

    assert_eq!(m.file_bytes, len);
    assert!(m.object_tree_estimate >= 4_000_000, "{m:?}");
    assert!(m.object_tree_estimate <= len * 3, "{m:?}");
    // The only allocation is the walk stack Vec (pointers to pending
    // objects; grows with the widest dict/array, doubling). 64 KiB is far
    // above what this document needs and far below its size.
    assert!(
        allocated < 64 * 1024,
        "memory_usage allocated {allocated} bytes"
    );
}
