//! Native stack guard for compiler-backed LSP requests.

// Windows executables reserve a 1 MiB stack by default. Compiler queries can
// legitimately need more than that even for small multi-module workspaces, so
// enter a larger stack segment before starting native analysis. This boundary
// must precede Salsa queries: Windows grows via Fibers, and switching inside an
// active query separates Salsa's attached-database thread-local from the query.
// The red zone also covers Tokio's relatively small worker stacks without
// making the common Unix 8 MiB stack allocate another segment.
#[cfg(not(target_arch = "wasm32"))]
const ANALYSIS_STACK_RED_ZONE: usize = 2 * 1024 * 1024;
#[cfg(not(target_arch = "wasm32"))]
const ANALYSIS_STACK_SIZE: usize = 8 * 1024 * 1024;

pub(crate) fn with_analysis_stack<T>(analysis: impl FnOnce() -> T) -> T {
    #[cfg(not(target_arch = "wasm32"))]
    {
        stacker::maybe_grow(ANALYSIS_STACK_RED_ZONE, ANALYSIS_STACK_SIZE, analysis)
    }

    #[cfg(target_arch = "wasm32")]
    {
        analysis()
    }
}

#[cfg(test)]
pub(crate) fn on_test_stack(stack_size: usize, test: fn()) {
    let result = std::thread::Builder::new()
        .stack_size(stack_size)
        .spawn(test)
        .expect("spawn constrained-stack LSP test")
        .join();
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
