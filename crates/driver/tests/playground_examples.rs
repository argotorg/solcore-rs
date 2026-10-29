//! Keep the normal playground's source examples compatible with the compiler.

#[path = "support/example_sources.rs"]
mod example_sources;

#[test]
fn playground_examples_compile() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    example_sources::check_example_sources(&root, "playground/src/examples", 16);
}
