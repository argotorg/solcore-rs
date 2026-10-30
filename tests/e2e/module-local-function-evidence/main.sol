pragma solcore;

import { uint256, Proxy, string } from std;
import * from std.dispatch;
import { a, bothA, valueA } from a;
import { b, bothB, valueB } from b;

function concatLit(x: word) returns (word) {
    return x;
}

contract ModuleLocalEvidence {
    // #[() -> 3]
    function direct() public returns (uint256) {
        return uint256(primAddWord(a(), b()));
    }
    // #[() -> 33]
    function nested() public returns (uint256) {
        return uint256(primAddWord(bothA(), bothB()));
    }
    // #[() -> 3]
    function values() public returns (uint256) {
        return uint256(primAddWord(valueA(), valueB()));
    }
    // #[() -> 7]
    function userConcat() public returns (uint256) {
        return uint256(concatLit(7));
    }
    // #[(1,2,3,4,5,6,7,8,9) -> 9]
    function ninth(a: uint256, b: uint256, c: uint256, d: uint256, e: uint256, f: uint256, g: uint256, h: uint256, i: uint256) public returns (uint256) {
        return i;
    }
}
