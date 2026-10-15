import * from std;
import * from std.dispatch;

// #[scenario("WithFallback")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x85bb7d69, label = "answer()(uint256)") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0x, label = "no selector -- user-defined fallback runs and reverts") -> revert(0x66616c6c6261636b2d7761732d63616c6c6564)]
// #[raw(0xdeadc0de, label = "unknown selector -- user-defined fallback runs and reverts") -> revert(0x66616c6c6261636b2d7761732d63616c6c6564)]
// #[raw(0xdeadc0de, value = 42, label = "unknown selector with value -- payable fallback runs and reverts") -> revert(0xb5988ea3)]
contract WithFallback {
    constructor() {}

    function answer() public returns (uint256) {
        return uint256(42);
    }

    fallback() {
        revertLit("fallback-was-called");
    }
}
