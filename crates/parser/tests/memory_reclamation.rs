use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
};

use hir::input::SourceFile;
use solcore_parser::{parse_diagnostics, parse_file_to_hir};

struct CountingAllocator;

static LIVE_REQUESTED_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            LIVE_REQUESTED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            LIVE_REQUESTED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_REQUESTED_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size >= layout.size() {
                LIVE_REQUESTED_BYTES.fetch_add(new_size - layout.size(), Ordering::Relaxed);
            } else {
                LIVE_REQUESTED_BYTES.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

#[salsa::db]
#[derive(Default)]
struct TestDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::db]
impl hir::Db for TestDb {
    fn def_location_table<'db>(
        &'db self,
        file: SourceFile,
    ) -> &'db hir::anchor::DefLocationTable<'db> {
        parse_file_to_hir(self, file).def_locations(self)
    }
}

#[salsa::db]
impl solcore_parser::Db for TestDb {}

const SOURCE: &str = "pragma solcore;\nfunction id(x: word) -> word { return x; }\n";

fn parse_with_fresh_database() {
    let db = TestDb::default();
    let url = "memory:///memory-reclamation.solc"
        .parse()
        .expect("test URL is valid");
    let file = SourceFile::new(&db, url, Some(SOURCE.to_owned()));

    std::hint::black_box(parse_file_to_hir(&db, file).module(&db));
    std::hint::black_box(parse_diagnostics(&db, file).len());
}

#[test]
fn fresh_parser_databases_release_their_parser_graphs() {
    // Persistent-mode fuzzing repeats this lifecycle in one process. Warm up
    // process-wide lazy state before measuring whether each fresh database and
    // its parser combinators are actually reclaimed.
    for _ in 0..10 {
        parse_with_fresh_database();
    }

    let baseline = LIVE_REQUESTED_BYTES.load(Ordering::SeqCst);
    for _ in 0..20 {
        parse_with_fresh_database();
    }
    let end = LIVE_REQUESTED_BYTES.load(Ordering::SeqCst);
    let retained = end.saturating_sub(baseline);

    // Allow a little noise from the test runtime and dependency-level lazy
    // caches. A leaked parser graph retains roughly 100 KiB per iteration, so
    // that regression exceeds this bound by more than an order of magnitude.
    assert!(
        retained <= 64 * 1024,
        "fresh parser databases retained {retained} requested heap bytes across 20 iterations"
    );
}
