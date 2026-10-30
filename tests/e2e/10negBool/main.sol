pragma solcore;

trait Neg<a> {
    function neg(x: a) returns (a);
}

enum B {
    F,
    T
}

impl Neg<B> {
    function neg(x: B) returns (B) {
        match (x) {
            case B.F {
                return B.T;
            }
            case B.T {
                return B.F;
            }
        }
    }
}

contract NegBool {

    function fromB(b: B) returns (word) {
        match (b) {
            case B.F {
                return 0;
            }
            case B.T {
                return 1;
            }
        }
    }

    // #[() -> 1]
    function run() public returns (uint256) {
        return uint256(fromB(Neg.neg(B.F)));
    }
}
import * from std;
import * from std.dispatch;
