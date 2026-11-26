pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: forbid instantiation-depth-exceeded
//~ specialize: forbid missing-evidence

import * from std;

trait A<t> {
    function fa(x: t) returns (word);
}

trait B<t> where t: A {}

impl A<uint256> {
    function fa(x: uint256) returns (word) {
        return Typedef.rep(x);
    }
}

impl B<uint256> {}

// The recursive call builds the `uint256: B` dictionary inside a function that
// holds one. Both must be the same dictionary, or every level of the recursion
// is a new specialization.
function walk<p>(x: p, n: uint256) returns (word) where p: B {
    if (n == 0) {
        return A.fa(x);
    }
    return walk(n, n - 1);
}

function main(n: uint256) returns (word) {
    return walk(n, n);
}
