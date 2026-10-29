pragma solcore;

import * from std;
import * from std.signed;
import * from std.dispatch;

// These locals have no explicit numeric type or typed numeric context.
// The bool result keeps the test from forcing uint256 through its return type.
function implicitSuccess() returns (bool) {
    let value = 6;
    value *= 7;
    return value == 42;
}

function negateSigned(value: int256) returns (int256) {
    return -value;
}

function negateWord(value: word) returns (word) {
    return -value;
}

contract Issue92CheckedDefaults {
    // Inferred numeric scrutinees and typed uint256 parameters share patterns.
    // #[(0) -> (10, 10)]
    // #[(1) -> (20, 20)]
    // #[(2) -> (30, 30)]
    function runtimePatterns(choice: uint256) public returns (uint256, uint256) {
        let value = 0;
        if (choice == 1) {
            value = 1;
        }
        if (choice == 2) {
            value = 2;
        }
        let inferred: word;
        match (value) {
            case 0 {
                inferred = 10;
            }
            case 1 {
                inferred = 20;
            }
            case _ {
                inferred = 30;
            }
        }
        let typed: word;
        match (choice) {
            case 0 {
                typed = 10;
            }
            case 1 {
                typed = 20;
            }
            case _ {
                typed = 30;
            }
        }
        return (uint256(inferred), uint256(typed));
    }

    // Runtime branches prevent the invalid arithmetic from being precomputed.
    // #[(false) -> true]
    // #[(true) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function implicitAdd(fail: bool) public returns (bool) {
        let x = 41;
        if (fail) {
            x = 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff;
        }
        let result = x + 1;
        return result == 42;
    }

    // #[(false) -> true]
    // #[(true) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function implicitSub(fail: bool) public returns (bool) {
        let x = 1;
        if (fail) {
            x = 0;
        }
        let result = x - 1;
        return result == 0;
    }

    // #[(false) -> true]
    // #[(true) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function implicitMul(fail: bool) public returns (bool) {
        let x = 21;
        if (fail) {
            x = 0x8000000000000000000000000000000000000000000000000000000000000000;
        }
        let result = x * 2;
        return result == 42;
    }

    // #[(false) -> true]
    // #[(true) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000012)]
    function implicitDiv(fail: bool) public returns (bool) {
        let divisor = 2;
        if (fail) {
            divisor = 0;
        }
        let result = 84 / divisor;
        return result == 42;
    }

    // #[(false) -> true]
    // #[(true) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000012)]
    function implicitMod(fail: bool) public returns (bool) {
        let divisor = 5;
        if (fail) {
            divisor = 0;
        }
        let result = 42 % divisor;
        return result == 2;
    }

    // #[(false) -> true]
    // #[(true) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function implicitNeg(fail: bool) public returns (bool) {
        let x = 0;
        if (fail) {
            x = 1;
        }
        let result = -x;
        return result == 0;
    }

    // Each nonzero selector reaches a different checked compound operation.
    // #[(0) -> true]
    // #[(1) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    // #[(2) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    // #[(3) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    // #[(4) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000012)]
    // #[(5) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000012)]
    function implicitCompound(failure: uint256) public returns (bool) {
        let x = 6;
        if (failure == 1) {
            x = 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff;
        }
        x += 1;
        if (failure == 2) {
            x = 0;
        }
        x -= 1;
        if (failure == 3) {
            x = 0x8000000000000000000000000000000000000000000000000000000000000000;
        }
        x *= 7;
        let divisor = 1;
        if (failure == 4) {
            divisor = 0;
        }
        x /= divisor;
        if (failure == 5) {
            divisor = 0;
        }
        x %= divisor;
        return x == 0;
    }

    // The same inferred arithmetic and unary operators also evaluate at comptime.
    // #[() -> true]
    function comptimeSuccess() public returns (bool) {
        let inferred: comptime<bool> = implicitSuccess();
        let signed: comptime<int256> = negateSigned(-42);
        let raw: comptime<word> = negateWord(1);
        let minDecimal: comptime<int8> = -128;
        let minHex: comptime<int8> = -0x80;
        return inferred && Typedef.rep(signed) == 42 && raw == 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
        && minDecimal == minHex && Typedef.rep(minDecimal) == 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80;
    }

    // Explicit word annotations retain EVM wrapping, including unary minus.
    // #[(0) -> (1, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0)]
    // #[(1) -> (2, 0, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff)]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> (0, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe, 1)]
    function explicitWord(raw: uint256) public returns (uint256, uint256, uint256) {
        let x: word = Typedef.rep(raw);
        return (uint256(x + 1), uint256(x - 1), uint256(-x));
    }

    // Checked unsigned negation permits zero and rejects a negative result.
    // #[(0) -> 0]
    // #[(1) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function negUnsigned(x: uint256) public returns (uint256) {
        return -x;
    }

    // Raw ABI words keep these cases focused on negation rather than codecs.
    // Every signed width covers zero, -1, MAX, and the overflowing MIN.
    // #[(0) -> 0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 1]
    // #[(127) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff81]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function negI8(raw: uint256) public returns (uint256) {
        let x: int8 = Typedef.abs(Typedef.rep(raw));
        return uint256(Typedef.rep(-x));
    }

    // #[(0) -> 0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 1]
    // #[(32767) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8001]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function negI16(raw: uint256) public returns (uint256) {
        let x: int16 = Typedef.abs(Typedef.rep(raw));
        return uint256(Typedef.rep(-x));
    }

    // #[(0) -> 0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 1]
    // #[(2147483647) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000001]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffff80000000) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function negI32(raw: uint256) public returns (uint256) {
        let x: int32 = Typedef.abs(Typedef.rep(raw));
        return uint256(Typedef.rep(-x));
    }

    // #[(0) -> 0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 1]
    // #[(9223372036854775807) -> 0xffffffffffffffffffffffffffffffffffffffffffffffff8000000000000001]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffff8000000000000000) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function negI64(raw: uint256) public returns (uint256) {
        let x: int64 = Typedef.abs(Typedef.rep(raw));
        return uint256(Typedef.rep(-x));
    }

    // #[(0) -> 0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 1]
    // #[(0x7fffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffff80000000000000000000000000000001]
    // #[(0xffffffffffffffffffffffffffffffff80000000000000000000000000000000) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function negI128(raw: uint256) public returns (uint256) {
        let x: int128 = Typedef.abs(Typedef.rep(raw));
        return uint256(Typedef.rep(-x));
    }

    // #[(0) -> 0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 1]
    // #[(0x7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x8000000000000000000000000000000000000000000000000000000000000001]
    // #[(0x8000000000000000000000000000000000000000000000000000000000000000) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function negI256(raw: uint256) public returns (uint256) {
        let x: int256 = Typedef.abs(Typedef.rep(raw));
        return uint256(Typedef.rep(-x));
    }
}
