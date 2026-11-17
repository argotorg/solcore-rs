pragma solcore;

trait D<t> {
    function d(x: t) returns (word);
}

// The trait method has its own type parameter with a constraint.
trait C<t> {
    function c<b>(x: t, y: b) returns (word) where b: D;
}

enum T { T }

impl D<word> {
    function d(x: word) returns (word) {
        return x;
    }
}

impl C<T> {
    function c<b>(x: T, y: b) returns (word) where b: D {
        return D.d(y);
    }
}

contract K {
    function main() public returns (word) {
        let w: word = 3;
        return C.c(T, w);
    }
}
