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

trait C<t> where t: A {}

impl A<uint256> {
    function fa(x: uint256) returns (word) {
        return Typedef.rep(x);
    }
}

impl B<uint256> {}

impl C<uint256> {}

// Sibling traits: each function builds the dictionary the other one takes,
// while it holds one that shares the superclass `A`.
function left<t>(x: t, n: uint256) returns (word) where t: B {
    if (n == 0) {
        return A.fa(x);
    }
    return right(n, n - 1);
}

function right<t>(x: t, n: uint256) returns (word) where t: C {
    if (n == 0) {
        return A.fa(x);
    }
    return left(n, n - 1);
}

function main(n: uint256) returns (word) {
    return left(n, n);
}
