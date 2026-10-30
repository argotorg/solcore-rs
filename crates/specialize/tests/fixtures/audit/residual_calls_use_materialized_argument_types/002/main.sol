pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean

import * from std;
function opaqueAdd(left: word, right: word) returns (word) {
    assembly {
        mstore(0, calldatasize())
        left := add(left, right)
    }
    return left;
}
function main(input: word) returns (word) {
    let seed: comptime<word> = 7;
    for (; false;) {}
    return primAddWord(seed, input);
}
