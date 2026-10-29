pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: entry-return 7

import * from std;
import * from std.dispatch;
function touch() {
    assembly {
        mstore(0, calldatasize())
    }
}
function opaque(flag: bool) returns (bool) {
    touch();
    return flag;
}
contract C {
    seed: word;
    function overwrite() {
        seed = 9;
    }
    function main() returns (word) {
        let flag = opaque(false);
        let seed: comptime<word> = 7;
        assembly {
            mstore(0, calldatasize())
        }
        return seed;
    }
}
