pragma solcore;

// An integer literal that the `Int` impl of its type does not convert is one
// error at the literal (#179).
import * from std;
import * from std.unsigned;
import * from std.signed;
import * from std.dispatch;

// A user type whose `Int` impl rejects odd integers.
enum Even {
    Even(uint256)
}

impl Int<Even> {
    function fromInteger(comptime x: integer) returns (Even) {
        let value: comptime<uint256> = Int.fromInteger(x);
        if (value % 2 == 1) {
            panic(0x01);
        }
        return Even(value);
    }
}

function half(e: Even) returns (uint256) {
    match (e) {
        case Even(value) {
            return value / 2;
        }
    }
}

// Both specializations hold the same literal.
function widen<t>(x: t) returns (uint256) where t: Typedef<word> {
    let limit: uint8 = 256;
    return uint256(Typedef.rep(x)) + uint256(Typedef.rep(limit));
}

contract C {
    function unsigned() public returns (uint256) {
        let x: uint8 = 300;
        return uint256(Typedef.rep(x));
    }

    function signed() public returns (uint256) {
        let x: int8 = -129;
        return 1;
    }

    function full() public returns (uint256) {
        return 115792089237316195423570985008687907853269984665640564039457584007913129639936;
    }

    function operand(x: uint8) public returns (bool) {
        return x < 0x100;
    }

    function user() public returns (uint256) {
        return half(4) + half(7);
    }

    function generic(a: uint8, b: uint16) public returns (uint256) {
        return widen(a) + widen(b);
    }
}
