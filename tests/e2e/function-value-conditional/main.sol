pragma solcore;

// A conditional expression may choose between function values. It is
// compiled like the same choice written with `if` and an assignment (#177).
import * from std;
import * from std.dispatch;

function inc(v: uint256) returns (uint256) {
    return v + 1;
}

function dbl(v: uint256) returns (uint256) {
    return v * 2;
}

function sq(v: uint256) returns (uint256) {
    return v * v;
}

function apply(f: function (uint256) returns (uint256), x: uint256) returns (uint256) {
    return f(x);
}

function applyLast(x: uint256, f: function (uint256) returns (uint256)) returns (uint256) {
    return f(x);
}

contract FunctionValueConditional {
    trace: uint256;

    function step(k: uint256) returns (uint256) {
        trace = trace * 10 + k;
        return k;
    }

    function flag(c: bool) returns (bool) {
        trace = trace * 10 + 9;
        return c;
    }

    // #[(true, 5) -> 6]
    // #[(false, 5) -> 10]
    function declared(c: bool, x: uint256) public returns (uint256) {
        let f: function (uint256) returns (uint256) = c ? inc : dbl;
        return f(x);
    }

    // #[(true, 5) -> 10]
    // #[(false, 5) -> 25]
    function assigned(c: bool, x: uint256) public returns (uint256) {
        let f: function (uint256) returns (uint256) = inc;
        f = c ? dbl : sq;
        return f(x);
    }

    // #[(true, false, 5) -> 6]
    // #[(false, true, 5) -> 10]
    // #[(false, false, 5) -> 25]
    function nested(a: bool, b: bool, x: uint256) public returns (uint256) {
        let f: function (uint256) returns (uint256) = a ? inc : (b ? dbl : sq);
        return f(x);
    }

    // #[(true, 3, 5) -> 8]
    // #[(false, 3, 5) -> 10]
    function capturing(c: bool, k: uint256, x: uint256) public returns (uint256) {
        let f: function (uint256) returns (uint256) = c ? lam (v: uint256) -> uint256 {
            return v + k;
        } : dbl;
        return apply(f, x);
    }

    // #[(true, 5) -> 6]
    // #[(false, 5) -> 10]
    function argument(c: bool, x: uint256) public returns (uint256) {
        return apply(c ? inc : dbl, x);
    }

    // #[(true, 5) -> 6]
    // #[(false, 5) -> 10]
    function called(c: bool, x: uint256) public returns (uint256) {
        return (c ? inc : dbl)(x);
    }

    // The conditional comes after a literal and after another call.
    // #[(true, 5) -> 107]
    // #[(false, 5) -> 112]
    function laterOperand(c: bool, x: uint256) public returns (uint256) {
        return 100 + applyLast(inc(x), c ? inc : dbl);
    }

    // #[(true, false, 5) -> 31]
    // #[(false, true, 5) -> 16]
    function twice(a: bool, b: bool, x: uint256) public returns (uint256) {
        return apply(a ? inc : dbl, x) + apply(b ? inc : sq, x);
    }

    // #[(true, 5) -> 1]
    // #[(false, 5) -> 2]
    function condition(c: bool, x: uint256) public returns (uint256) {
        if (apply(c ? inc : dbl, x) < 8) {
            return 1;
        }
        return 2;
    }

    // #[(true, 5) -> 18]
    // #[(false, 5) -> 30]
    function repeated(c: bool, x: uint256) public returns (uint256) {
        let total: uint256 = 0;
        for (let i: uint256 = 0; i < 3; i = i + 1) {
            total = total + apply(c ? inc : dbl, x);
        }
        return total;
    }

    // Operands keep their order: `step(3)`, the condition, then `step(2)`.
    // #[(true) -> (392, 6)]
    // #[(false) -> (392, 8)]
    function ordered(c: bool) public returns (uint256, uint256) {
        trace = 0;
        let r: uint256 = applyLast(step(3), flag(c) ? inc : dbl) + step(2);
        return (trace, r);
    }
}
