pragma solcore;

import * from std;
import * from std.Generic;

trait A<t> {
    function fa(x: t) returns (word);
}

trait B<t> where t: A {
    function fb(x: t) returns (word);
}

enum T {
    T
}

// `T: B` can be used as `T: A`, but no impl provides `T: A`.
impl B<T> {
    function fb(x: T) returns (word) {
        return 1;
    }
}

enum Box<a> {
    Box(a)
}

impl<a> A<Box<a>> where a: A {
    function fa(x: Box<a>) returns (word) {
        return 2;
    }
}

// The superclass follows from the condition: `a: B` gives `a: A`, and the
// impl above turns that into `Box<a>: A`.
impl<a> B<Box<a>> where a: B {
    function fb(x: Box<a>) returns (word) {
        return 3;
    }
}

enum Pair<a> {
    Pair(a, a)
}

// Nothing provides `Pair<a>: A`, even under the condition.
impl<a> B<Pair<a>> where a: B {
    function fb(x: Pair<a>) returns (word) {
        return 4;
    }
}

trait D<t> where t: A {}

// For a bare type parameter the superclass has to come from the condition.
impl<a> D<a> {}

// The derived `Ord` impl needs `Color: Eq`, which is not derived.
#[derive(Ord)]
enum Color {
    Red,
    Green
}
