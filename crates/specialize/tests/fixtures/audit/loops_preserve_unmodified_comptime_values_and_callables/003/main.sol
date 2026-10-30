pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: entry-return 7

function main(flag: bool) returns (word) {
    let seed: comptime<word> = 7;
    for (; flag;) {
        assembly {
            mstore(0, 99)
        }
        break;
    }
    return seed;
}
