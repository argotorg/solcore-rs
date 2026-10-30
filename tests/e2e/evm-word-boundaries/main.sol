pragma solcore;

import * from std;
import * from std.dispatch;

contract EvmWordBoundaries {
    // #[(0x0, 0x0) -> 0x0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0x0) -> 0x0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x1]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0x1) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    // #[(0x1, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    function unsignedDivide(a: uint256, b: uint256) public returns (uint256) {
        let aWord: word = Typedef.rep(a);
        let bWord: word = Typedef.rep(b);
        let result: word;
        assembly {
            result := div(aWord, bWord)
        }
        return uint256(result);
    }

    // #[(0x8000000000000000000000000000000000000000000000000000000000000000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x8000000000000000000000000000000000000000000000000000000000000000]
    // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 0x2) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd]
    // #[(0x7, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe) -> 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd]
    // #[(0x8000000000000000000000000000000000000000000000000000000000000000, 0x1) -> 0x8000000000000000000000000000000000000000000000000000000000000000]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0x0) -> 0x0]
    function signedDivide(a: uint256, b: uint256) public returns (uint256) {
        let aWord: word = Typedef.rep(a);
        let bWord: word = Typedef.rep(b);
        let result: word;
        assembly {
            result := sdiv(aWord, bWord)
        }
        return uint256(result);
    }

    // #[(0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9, 0x3) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    // #[(0x7, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffd) -> 0x1]
    // #[(0x8000000000000000000000000000000000000000000000000000000000000000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0x0) -> 0x0]
    function signedRemainder(a: uint256, b: uint256) public returns (uint256) {
        let aWord: word = Typedef.rep(a);
        let bWord: word = Typedef.rep(b);
        let result: word;
        assembly {
            result := smod(aWord, bWord)
        }
        return uint256(result);
    }

    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe) -> 0x2]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0x1, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x1]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0x0) -> 0x0]
    // #[(0x8000000000000000000000000000000000000000000000000000000000000000, 0x8000000000000000000000000000000000000000000000000000000000000000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x1]
    function addModulo(a: uint256, b: uint256, c: uint256) public returns (uint256) {
        let aWord: word = Typedef.rep(a);
        let bWord: word = Typedef.rep(b);
        let cWord: word = Typedef.rep(c);
        let result: word;
        assembly {
            result := addmod(aWord, bWord, cWord)
        }
        return uint256(result);
    }

    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe) -> 0x1]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0x0) -> 0x0]
    // #[(0x8000000000000000000000000000000000000000000000000000000000000000, 0x8000000000000000000000000000000000000000000000000000000000000000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x4000000000000000000000000000000000000000000000000000000000000000]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0x61) -> 0xb]
    function multiplyModulo(a: uint256, b: uint256, c: uint256) public returns (uint256) {
        let aWord: word = Typedef.rep(a);
        let bWord: word = Typedef.rep(b);
        let cWord: word = Typedef.rep(c);
        let result: word;
        assembly {
            result := mulmod(aWord, bWord, cWord)
        }
        return uint256(result);
    }

    // #[(0x0, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    // #[(0xff, 0x1) -> 0x8000000000000000000000000000000000000000000000000000000000000000]
    // #[(0x100, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    // #[(0x8000000000000000000000000000000000000000000000000000000000000000, 0x1) -> 0x0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    function shiftLeft(a: uint256, b: uint256) public returns (uint256) {
        let aWord: word = Typedef.rep(a);
        let bWord: word = Typedef.rep(b);
        let result: word;
        assembly {
            result := shl(aWord, bWord)
        }
        return uint256(result);
    }

    // #[(0x0, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    // #[(0xff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x1]
    // #[(0x100, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    // #[(0x8000000000000000000000000000000000000000000000000000000000000000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    function shiftRight(a: uint256, b: uint256) public returns (uint256) {
        let aWord: word = Typedef.rep(a);
        let bWord: word = Typedef.rep(b);
        let result: word;
        assembly {
            result := shr(aWord, bWord)
        }
        return uint256(result);
    }

    // #[(0x0, 0x8000000000000000000000000000000000000000000000000000000000000000) -> 0x8000000000000000000000000000000000000000000000000000000000000000]
    // #[(0xff, 0x8000000000000000000000000000000000000000000000000000000000000000) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    // #[(0x100, 0x8000000000000000000000000000000000000000000000000000000000000000) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    // #[(0x8000000000000000000000000000000000000000000000000000000000000000, 0x8000000000000000000000000000000000000000000000000000000000000000) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0x1) -> 0x0]
    function shiftArithmetic(a: uint256, b: uint256) public returns (uint256) {
        let aWord: word = Typedef.rep(a);
        let bWord: word = Typedef.rep(b);
        let result: word;
        assembly {
            result := sar(aWord, bWord)
        }
        return uint256(result);
    }

    // #[(0x0, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xff]
    // #[(0x1f, 0x1234) -> 0x34]
    // #[(0x20, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    // #[(0x8000000000000000000000000000000000000000000000000000000000000000, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    function extractByte(a: uint256, b: uint256) public returns (uint256) {
        let aWord: word = Typedef.rep(a);
        let bWord: word = Typedef.rep(b);
        let result: word;
        assembly {
            result := byte(aWord, bWord)
        }
        return uint256(result);
    }

    // #[(0x0) -> 0x100]
    // #[(0x1) -> 0xff]
    // #[(0x8000000000000000000000000000000000000000000000000000000000000000) -> 0x0]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0x0]
    // #[(0x100000000000000000000000000000000) -> 0x7f]
    function leadingZeros(a: uint256) public returns (uint256) {
        let aWord: word = Typedef.rep(a);
        let result: word;
        assembly {
            result := clz(aWord)
        }
        return uint256(result);
    }

    // #[(0x0, 0x80) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff80]
    // #[(0x0, 0xff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    // #[(0x0, 0x7f) -> 0x7f]
    // #[(0x1, 0x8000) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8000]
    // #[(0x1f, 0x8000000000000000000000000000000000000000000000000000000000000000) -> 0x8000000000000000000000000000000000000000000000000000000000000000]
    // #[(0x20, 0x1) -> 0x1]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    function signExtend(a: uint256, b: uint256) public returns (uint256) {
        let aWord: word = Typedef.rep(a);
        let bWord: word = Typedef.rep(b);
        let result: word;
        assembly {
            result := signextend(aWord, bWord)
        }
        return uint256(result);
    }

}
