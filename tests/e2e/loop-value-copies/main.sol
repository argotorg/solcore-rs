pragma solcore;

import * from std;
import * from std.dispatch;

contract LoopValueCopies {
    // Copies retain the old counter value across the loop backedge.
    // #[(0) -> 0]
    // #[(1) -> 0]
    // #[(3) -> 2]
    function previous(count: uint256) public returns (uint256) {
        let previous: uint256 = uint256(0);
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            previous = i;
        }
        return previous;
    }

    // Three loop-carried assignments form a parallel-copy cycle.
    // #[(0) -> 123]
    // #[(1) -> 231]
    // #[(2) -> 312]
    // #[(3) -> 123]
    function rotate(count: uint256) public returns (uint256) {
        let a: word = 1;
        let b: word = 2;
        let c: word = 3;
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            let previous: word = a;
            a = b;
            b = c;
            c = previous;
        }
        let result: word;
        assembly {
            result := add(add(mul(a, 100), mul(b, 10)), c)
        }
        return uint256(result);
    }

    // Inline assembly reaches the same backend parallel-copy contract.
    // #[(0) -> 123]
    // #[(1) -> 231]
    // #[(2) -> 312]
    // #[(3) -> 123]
    function assemblyRotate(count: uint256) public returns (uint256) {
        let limit: word = Typedef.rep(count);
        let result: word;
        assembly {
            let a := 1
            let b := 2
            let c := 3
            for {
                let i := 0
            } lt(i, limit) {
                i := add(i, 1)
            } {
                let previous := a
                a := b
                b := c
                c := previous
            }
            result := add(add(mul(a, 100), mul(b, 10)), c)
        }
        return uint256(result);
    }

    // Twenty live values force memory spills as well as stack copies.
    // #[(0) -> 120]
    // #[(1) -> 201]
    // #[(3) -> 403]
    // #[(21) -> 201]
    function wideRotate(count: uint256) public returns (uint256) {
        let a0: word = 1;
        let a1: word = 2;
        let a2: word = 3;
        let a3: word = 4;
        let a4: word = 5;
        let a5: word = 6;
        let a6: word = 7;
        let a7: word = 8;
        let a8: word = 9;
        let a9: word = 10;
        let a10: word = 11;
        let a11: word = 12;
        let a12: word = 13;
        let a13: word = 14;
        let a14: word = 15;
        let a15: word = 16;
        let a16: word = 17;
        let a17: word = 18;
        let a18: word = 19;
        let a19: word = 20;
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            let previous: word = a0;
            a0 = a1;
            a1 = a2;
            a2 = a3;
            a3 = a4;
            a4 = a5;
            a5 = a6;
            a6 = a7;
            a7 = a8;
            a8 = a9;
            a9 = a10;
            a10 = a11;
            a11 = a12;
            a12 = a13;
            a13 = a14;
            a14 = a15;
            a15 = a16;
            a16 = a17;
            a17 = a18;
            a18 = a19;
            a19 = previous;
        }
        let result: word;
        assembly {
            result := add(mul(a0, 100), a19)
        }
        return uint256(result);
    }
}
