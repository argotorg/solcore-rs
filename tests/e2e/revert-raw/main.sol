import * from std;
import * from std.dispatch;

function my_revert() returns (word) {
    revertLit("regression");
    return 0;
}

// #[scenario("Foo")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x08ad856f, label = "noAnswer()") -> revert(0x72656772657373696f6e)]
// #[raw(0x85bb7d69, label = "answer()(uint256)") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
contract Foo {
    constructor() {}

    function noAnswer() public returns (uint256) {
        return uint256(my_revert());
    }

    function answer() public returns (uint256) {
        return uint256(42);
    }
}
