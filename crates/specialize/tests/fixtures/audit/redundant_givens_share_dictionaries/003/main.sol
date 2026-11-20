pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: forbid missing-evidence

import * from std;

trait A<t> {
    function fa(x: t) returns (word);
}

trait B<t> where t: A {}

trait C<t> where t: A {}

enum Box<a> {
    Box(a)
}

impl<a> A<Box<a>> where a: A {
    function fa(x: Box<a>) returns (word) {
        match (x) {
            case Box(inner) {
                return A.fa(inner);
            }
        }
    }
}

impl A<uint256> {
    function fa(x: uint256) returns (word) {
        return Typedef.rep(x);
    }
}

impl B<uint256> {}

impl C<uint256> {}

function both<t>(x: t) returns (word) where t: C, t: B {
    return primAddWord(A.fa(x), A.fa(Box(x)));
}

function main(n: uint256) returns (word) {
    return both(n);
}
