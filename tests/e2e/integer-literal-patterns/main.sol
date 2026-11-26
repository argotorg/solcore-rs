pragma solcore;

// An integer literal pattern stands for `Int.fromInteger` of the literal, so
// it works on every type with an `Int` instance (#178).
import * from std;
import * from std.unsigned;
import * from std.dispatch;

function small(a: uint8) returns (uint256) {
    match (a) {
        case 0 {
            return 10;
        }
        case 255 {
            return 20;
        }
        case _ {
            return 30;
        }
    }
}

// A user type whose literals are scaled: `1` is one hundred cents.
enum Cents {
    Cents(uint256)
}

impl Int<Cents> {
    function fromInteger(comptime x: integer) returns (Cents) {
        let amount: comptime<uint256> = Int.fromInteger(x);
        return Cents(amount * 100);
    }
}

function price(c: Cents) returns (uint256) {
    match (c) {
        case 1 {
            return 1;
        }
        case 2 {
            return 2;
        }
        case _ {
            return 0;
        }
    }
}

// A literal nested in a constructor pattern.
enum Pair {
    Pair(uint8, uint16)
}

function nested(p: Pair) returns (uint256) {
    match (p) {
        case Pair.Pair(1, 2) {
            return 12;
        }
        case Pair.Pair(1, _) {
            return 1;
        }
        case _ {
            return 0;
        }
    }
}

// The `Int` instance comes from the caller.
function isZero<t>(a: t) returns (bool) where t: Int {
    match (a) {
        case 0 {
            return true;
        }
        case _ {
            return false;
        }
    }
}

contract IntegerLiteralPatterns {
    // #[(0) -> (true, true)]
    // #[(256) -> (true, false)]
    // #[(3) -> (false, false)]
    function generic(x: uint256) public returns (bool, bool) {
        let narrow: uint8 = Typedef.abs(Typedef.rep(x % 256));
        return (isZero(narrow), isZero(x));
    }

    // #[(0) -> 10]
    // #[(255) -> 20]
    // #[(7) -> 30]
    function uint8Pattern(a: uint256) public returns (uint256) {
        let narrow: uint8 = Typedef.abs(Typedef.rep(a));
        return small(narrow);
    }

    // #[(100) -> 1]
    // #[(200) -> 2]
    // #[(1) -> 0]
    function scaled(raw: uint256) public returns (uint256) {
        return price(Cents(raw));
    }

    // #[(1, 2) -> 12]
    // #[(1, 3) -> 1]
    // #[(2, 2) -> 0]
    function nestedPattern(a: uint256, b: uint256) public returns (uint256) {
        let first: uint8 = Typedef.abs(Typedef.rep(a));
        let second: uint16 = Typedef.abs(Typedef.rep(b));
        return nested(Pair.Pair(first, second));
    }
}
