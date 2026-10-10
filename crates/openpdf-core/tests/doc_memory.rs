//! `Document::memory_usage` must be cheap: no significant allocation.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(l.size(), Ordering::SeqCst);
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

    let before = ALLOCATED.load(Ordering::SeqCst);
    let m = doc.memory_usage();
    let allocated = ALLOCATED.load(Ordering::SeqCst) - before;
    eprintln!("large: file={len} {m:?} allocated_during_call={allocated}");

    assert_eq!(m.file_bytes, len);
    assert!(m.object_tree_estimate >= 4_000_000, "{m:?}");
    assert!(m.object_tree_estimate <= len * 3, "{m:?}");
    assert!(allocated < 4096, "memory_usage allocated {allocated} bytes");
}
