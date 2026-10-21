// `unchecked { ... }` is lexical: only operators written directly inside the
// block wrap. Operators after the block, in called functions, and in lambda
// bodies written inside the block stay checked and revert with Panic(0x11).
import * from std;
import * from std.unchecked.unsigned;
import * from std.dispatch;

function checkedSum(a: uint256, b: uint256) returns (uint256) {
    return a + b;
}

function apply(f: function (uint256) returns (uint256), x: uint256) returns (uint256) {
    return f(x);
}

contract UncheckedScope {
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 1) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function outside(a: uint256, b: uint256) public returns (uint256) {
        return a + b;
    }

    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 1) -> 0]
    // #[(2, 3) -> 5]
    function inside(a: uint256, b: uint256) public returns (uint256) {
        unchecked {
            return a + b;
        }
    }

    // #[(0, 1) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    function compoundInside(a: uint256, b: uint256) public returns (uint256) {
        let r: uint256 = a;
        unchecked {
            r -= b;
        }
        return r;
    }

    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 1) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function afterBlock(a: uint256, b: uint256) public returns (uint256) {
        let wrapped: uint256 = uint256(0);
        unchecked {
            wrapped = a + b;
        }
        return wrapped + a + b;
    }

    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 1) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function calledFunction(a: uint256, b: uint256) public returns (uint256) {
        unchecked {
            return checkedSum(a, b);
        }
    }

    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 1) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function lambdaBody(a: uint256, b: uint256) public returns (uint256) {
        unchecked {
            return apply(lam (v: uint256) -> uint256 {
                    return v + b;
                }, a);
        }
    }

    // #[(7, 0) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000012)]
    function divisionByZero(a: uint256, b: uint256) public returns (uint256) {
        unchecked {
            return a / b;
        }
    }
}
