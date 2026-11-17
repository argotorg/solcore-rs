pragma solcore;

// A type and a trait with the same name.
enum X { X(word) }

trait X<t> {
    function m(x: t) returns (word);
}

impl X<X> {
    function m(x: X) returns (word) {
        return 1;
    }
}

function g<t>(x: t) returns (word) where t: X {
    return X.m(x);
}
