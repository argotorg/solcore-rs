pragma solcore;

// A constructor called `{Type}_{Other}` shares its name with the lowered name
// of `Type.Other`. Each reference still selects its own constructor (#173).
import * from std;
import * from std.dispatch;

enum T {
    T_A,
    A
}

function isFirst(t: T) returns (bool) {
    match (t) {
        case T.T_A {
            return true;
        }
        default {
            return false;
        }
    }
}

function tag(t: T) returns (uint256) {
    match (t) {
        case T.T_A {
            return 1;
        }
        case T.A {
            return 2;
        }
    }
}

// The same pair of names in the other declaration order.
enum U {
    B,
    U_B
}

function tagU(u: U) returns (uint256) {
    match (u) {
        case U.B {
            return 1;
        }
        case U.U_B {
            return 2;
        }
    }
}

contract ConstructorNamedLikeQualified {
    // #[(true) -> false]
    // #[(false) -> true]
    function run(c: bool) public returns (bool) {
        let t: T = T.T_A;
        if (c) {
            t = T.A;
        }
        return isFirst(t);
    }

    // #[() -> (1, 2)]
    function both() public returns (uint256, uint256) {
        return (tag(T.T_A), tag(T.A));
    }

    // #[() -> (1, 2)]
    function otherOrder() public returns (uint256, uint256) {
        return (tagU(U.B), tagU(U.U_B));
    }
}
