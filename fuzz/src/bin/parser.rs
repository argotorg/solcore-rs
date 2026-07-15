use std::{panic, thread};

use solcore_fuzz::{COMPILER_STACK_SIZE, Target, process};

fn main() {
    let fuzzer = thread::Builder::new()
        .name("solcore-parser-fuzz".to_owned())
        .stack_size(COMPILER_STACK_SIZE)
        .spawn(|| afl::fuzz!(|input: &[u8]| process(Target::Parser, input)))
        .expect("failed to spawn parser fuzzing thread");
    if let Err(payload) = fuzzer.join() {
        panic::resume_unwind(payload);
    }
}
