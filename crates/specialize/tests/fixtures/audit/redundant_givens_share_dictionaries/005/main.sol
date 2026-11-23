pragma solcore;

pragma no-patterson-condition;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: forbid instantiation-depth-exceeded
//~ specialize: forbid missing-evidence

import * from std;

trait A<t> {
    function fa(x: t) returns (word);
}

trait P<t> {}

trait Q<t> {}

enum Foo {
    Foo
}

impl P<Foo> {}

impl Q<Foo> {}

enum Box<a> {
    Box(a)
}

// Both defaults apply to `Box<Foo>`, which has no unique impl of `A`.
default impl<a> A<Box<a>> where a: P {
    function fa(x: Box<a>) returns (word) {
        return 1;
    }
}

default impl<a> A<Box<a>> where a: Q {
    function fa(x: Box<a>) returns (word) {
        return 2;
    }
}

trait B<t> where t: A {
    function fb(x: t) returns (word);
}

trait C<t> where t: A {
    function fc(x: t) returns (word);
}

impl<a> B<Box<a>> where a: P {
    function fb(x: Box<a>) returns (word) {
        return 10;
    }
}

impl<a> C<Box<a>> where a: P {
    function fc(x: Box<a>) returns (word) {
        return 20;
    }
}

// The givens share the superclass `A`, which nothing uses. The dictionaries
// that the recursive call builds have no unique dictionary for it, at every
// level alike: they must not hold the ones at hand instead.
function walk<p>(x: p, n: uint256) returns (word) where p: B, p: C {
    if (n == 0) {
        return primAddWord(B.fb(x), C.fc(x));
    }
    return walk(Box.Box(Foo.Foo), n - 1);
}

function main(n: uint256) returns (word) {
    return walk(Box.Box(Foo.Foo), n);
}
