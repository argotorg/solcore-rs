pragma solcore;

trait C<t> {
    function c(x: t) returns (word);
}

trait D<t> {
    function d(x: t) returns (word);
}

enum Box<a> {
    Box(a)
}

// Callers of `C.c` prove only what the trait method declares, so nothing
// would provide `a: D` here.
impl<a> C<Box<a>> {
    function c(x: Box<a>) returns (word) where a: D {
        match (x) {
            case Box(inner) {
                return D.d(inner);
            }
        }
    }
}

trait E<t> where t: D {}

enum Pair<a> {
    Pair(a, a)
}

// `a: D` already follows from the impl's condition `a: E`.
impl<a> C<Pair<a>> where a: E {
    function c(x: Pair<a>) returns (word) where a: D {
        return 2;
    }
}

trait F<t> {
    function f<b>(x: t, y: b) returns (word) where b: D;
}

// Restating the trait method's constraint is accepted.
impl<a> F<Box<a>> {
    function f<b>(x: Box<a>, y: b) returns (word) where b: D {
        return D.d(y);
    }
}

// Restating the impl's own condition is accepted.
impl<a> D<Box<a>> where a: D {
    function d(x: Box<a>) returns (word) where a: D {
        return 3;
    }
}
