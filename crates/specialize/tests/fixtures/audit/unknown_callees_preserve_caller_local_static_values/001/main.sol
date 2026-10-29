pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: entry-return 7

function touch() {
    assembly {
        mstore(0, calldatasize())
    }
}
function main(flag: bool) returns (word) {
    let seed: comptime<word> = 7;
    for (; flag;) {
        touch();
        let observed: comptime<word> = seed;
        break;
    }
    touch();
    return seed;
}
