//! Command-line driver for parsing and resolving Solcore modules.
//!
//! The driver owns filesystem concerns: argument parsing, root selection,
//! loading reachable modules into the Salsa database, and rendering pull-style
//! diagnostics. Compiler crates stay pure and receive source files through
//! database inputs.

mod args;
mod db;
mod diagnostics;
mod emit;
mod modules;
mod paths;
mod pipeline;
mod trace;

use std::thread;

/// Stack size for the compilation thread. Recursive-descent parsing, HIR
/// lowering, and type folding recurse with input nesting depth; the default
/// main-thread stack overflows on deeply nested (but well-formed) programs.
const COMPILER_STACK_SIZE: usize = 256 * 1024 * 1024;

/// Entry point for the CLI driver.
///
/// Restores default SIGPIPE handling so piping output into e.g. `head` ends
/// the process instead of panicking, then runs the compiler on a thread with
/// a large stack.
fn main() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let result = thread::Builder::new()
        .name("solcore-compiler".to_owned())
        .stack_size(COMPILER_STACK_SIZE)
        .spawn(pipeline::run_compiler)
        .expect("spawn compiler thread")
        .join();
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
