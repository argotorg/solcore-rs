pragma solcore;

// An integer literal in a branch of `c ? x : y` takes the other branch's type
// whenever that type has an `Int` instance, and is converted by that
// instance's `fromInteger` exactly as `return 0;` would convert it.
import * from std;
import * from std.signed;
import * from std.unsigned;
import * from std.dispatch;

// Numeric only through its own `Int` instance, which stores centimeters, so
// a converted literal is distinguishable from a raw word.
struct Meters {
    centimeters: uint256;
}

impl Int<Meters> {
    function fromInteger(comptime x: integer) returns (Meters) {
        return Meters(Int.fromInteger(integerMul(x, 100)));
    }
}

function identity(x: uint256) returns (uint256) {
    return x;
}

function orNine<t>(c: bool, a: t) returns (t) where t: Int {
    return c ? a : 9;
}

contract ConditionalIntegerLiterals {
    counter: uint256;
    balances: mapping(uint256 => uint256);

    // #[(true, 7) -> 7]
    // #[(false, 7) -> 0]
    function elseLiteral(c: bool, a: uint256) public returns (uint256) {
        return c ? a : 0;
    }

    // #[(true, 7) -> 42]
    // #[(false, 7) -> 7]
    function thenLiteral(c: bool, a: uint256) public returns (uint256) {
        return c ? 42 : a;
    }

    // #[(true) -> 1]
    // #[(false) -> 2]
    function bothLiterals(c: bool) public returns (uint256) {
        return c ? 1 : 2;
    }

    // #[(true, 7) -> 255]
    // #[(false, 7) -> 7]
    function hexLiteral(c: bool, a: uint256) public returns (uint256) {
        return c ? 0xff : a;
    }

    // #[(true, true, 7) -> 7]
    // #[(true, false, 7) -> 1]
    // #[(false, true, 7) -> 2]
    function nested(c1: bool, c2: bool, a: uint256) public returns (uint256) {
        return c1 ? (c2 ? a : 1) : 2;
    }

    // #[(true, 7) -> 7]
    // #[(false, 7) -> 3]
    function callArgument(c: bool, a: uint256) public returns (uint256) {
        return identity(c ? a : 3);
    }

    // #[(true, 7) -> 7]
    // #[(false, 7) -> 4]
    function unannotatedLet(c: bool, a: uint256) public returns (uint256) {
        let x = c ? a : 4;
        return x;
    }

    // #[(true, 7) -> 5]
    // #[(false, 7) -> 7]
    function annotatedLet(c: bool, a: uint256) public returns (uint256) {
        let x: uint256 = c ? 5 : a;
        return x;
    }

    // #[(true, 7) -> 14]
    // #[(false, 7) -> 8]
    function plusOperand(c: bool, a: uint256) public returns (uint256) {
        return (c ? a : 1) + a;
    }

    // #[(true, 7) -> 7]
    // #[(false, 7) -> 255]
    function narrowUnsigned(c: bool, a: uint256) public returns (uint256) {
        let narrow: uint8 = Typedef.abs(Typedef.rep(a));
        let x = c ? narrow : 255;
        return uint256(Typedef.rep(x));
    }

    // #[(true, 7) -> 7]
    // #[(false, 7) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80]
    function narrowSigned(c: bool, a: uint256) public returns (uint256) {
        let narrow: int8 = Typedef.abs(Typedef.rep(a));
        let x = c ? narrow : -128;
        return uint256(Typedef.rep(x));
    }

    // #[(true, 7) -> 7]
    // #[(false, 7) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    function wideSigned(c: bool, a: uint256) public returns (uint256) {
        let wide: int256 = Typedef.abs(Typedef.rep(a));
        let x = c ? wide : -1;
        return uint256(Typedef.rep(x));
    }

    // #[(true, 7) -> 7]
    // #[(false, 7) -> 6]
    function rawWord(c: bool, a: uint256) public returns (uint256) {
        let w: word = Typedef.rep(a);
        let x = c ? w : 6;
        return uint256(x);
    }

    // #[(true, 7) -> 7]
    // #[(false, 7) -> 1100]
    function userInstance(c: bool, a: uint256) public returns (uint256) {
        let m = c ? Meters(a) : 11;
        return m.centimeters;
    }

    // #[(true, 7) -> 7]
    // #[(false, 7) -> 9]
    function constrainedParameter(c: bool, a: uint256) public returns (uint256) {
        return orNine(c, a);
    }

    // #[send(41)]
    function store(v: uint256) public {
        counter = v;
        balances[1] = v;
    }

    // #[(true) -> 41]
    // #[(false) -> 0]
    function storedOrZero(c: bool) public returns (uint256) {
        return c ? counter : 0;
    }

    // #[(true, 1) -> 41]
    // #[(false, 1) -> 10]
    function mappedOrTen(c: bool, k: uint256) public returns (uint256) {
        return c ? balances[k] : 10;
    }
}
