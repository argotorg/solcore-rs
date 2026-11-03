//! Check playground examples, including intentional errors in workshop starters.

#[path = "support/example_sources.rs"]
mod example_sources;

#[test]
fn playground_examples_compile() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    example_sources::check_example_sources(
        &root,
        "playground/src/examples",
        16,
        &[
            (
                "workshop-2/Token.sol",
                &["error[SC0302]: non-exhaustive pattern match"],
            ),
            (
                "workshop-3/Token.sol",
                &[
                    "error[SC0103]: undefined type constructor: Ownable",
                    "error[SC0101]: undefined name: ownedBy",
                    "error[SC0101]: undefined name: ownerOf",
                ],
            ),
        ],
    );
}
