//! Compile tutorial-owned sources independently of the playground catalog.

#[path = "support/example_sources.rs"]
mod example_sources;

#[test]
fn tutorial_examples_compile() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    example_sources::check_example_sources(&root, "solcore-tutorial/examples", 30);
}
