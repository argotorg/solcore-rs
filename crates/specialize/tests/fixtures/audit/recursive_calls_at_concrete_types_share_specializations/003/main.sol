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

trait D<t> where t: B {}

impl A<uint256> {
    function fa(x: uint256) returns (word) {
        return Typedef.rep(x);
    }
}

impl B<uint256> {}

impl D<uint256> {}

// A subtrait and its supertrait: `lower` holds a dictionary that `upper`'s
// constraint is a superclass of.
function upper<t>(x: t, n: uint256) returns (word) where t: B {
    if (n == 0) {
        return A.fa(x);
    }
    return lower(n, n - 1);
}

function lower<t>(x: t, n: uint256) returns (word) where t: D {
    if (n == 0) {
        return A.fa(x);
    }
    return upper(n, n - 1);
}

function main(n: uint256) returns (word) {
    return upper(n, n);
}
