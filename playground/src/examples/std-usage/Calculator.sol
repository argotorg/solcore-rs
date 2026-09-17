import * from std;
import * from std.dispatch;

contract Calculator {
    // The + operator is not compiler magic: it desugars to the same
    // standard-library trait method called explicitly below.
    // #[(20, 22) -> 42]
    // #[(0, 7) -> 7]
    function viaOperator(a: uint256, b: uint256) public returns (uint256) {
        return a + b;
    }

    // #[(20, 22) -> 42]
    // #[(0, 7) -> 7]
    function viaTrait(a: uint256, b: uint256) public returns (uint256) {
        return Num.add(a, b);
    }
}
