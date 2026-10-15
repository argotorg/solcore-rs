import * from std;
import * from std.dispatch;

// #[scenario("assembly")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x7d4bb2ec, label = "asmBool()(uint256) -> 1 (Yul `true` is word 1)") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
contract C {
    constructor() {}

    // Exercises a Yul block that declares an uninitialized `let y`, assigns the
    // boolean literal `true` to it, and writes it back to the surrounding
    // `word` local `x`. `true` is the word `1`, so this returns uint256(1).
    function asmBool() public returns (uint256) {
        let x : word;
        assembly {
          let y
          y := true
          x := y
        }
        return uint256(x);
    }
}
