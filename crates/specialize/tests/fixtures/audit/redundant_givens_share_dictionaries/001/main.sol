pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: forbid instantiation-depth-exceeded
//~ specialize: forbid missing-evidence

import * from std;

function walk<p, q>(x: p, y: q, n: uint256) returns (bool) where p: Ord, q: Num {
    if (n == 0) {
        return Ord.gt(x, x);
    }
    return walk(n, y, n - 1);
}

function main(n: uint256) returns (bool) {
    let b: uint256 = 2;
    return walk(n, b, n);
}
