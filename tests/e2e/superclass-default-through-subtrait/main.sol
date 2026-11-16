pragma solcore;

import * from std;
import * from std.dispatch;

// A subtrait impl does not hide the default impl of its superclass: `T: A`
// comes from the default, not from a projection out of `impl B<T>`.
trait A<t> {
    function fa(x: t) returns (word);
}

default impl<t> A<t> {
    function fa(x: t) returns (word) {
        return 7;
    }
}

trait B<t> where t: A {
    function fb(x: t) returns (word);
}

enum T {
    T
}

impl B<T> {
    function fb(x: T) returns (word) {
        return 1;
    }
}

enum Wrap<a> {
    Wrap(a)
}

impl<a> B<Wrap<a>> where a: A {
    function fb(x: Wrap<a>) returns (word) {
        return 2;
    }
}

function viaA<t>(x: t) returns (word) where t: A {
    return A.fa(x);
}

function viaB<t>(x: t) returns (word) where t: B {
    return primAddWord(A.fa(x), B.fb(x));
}

contract SuperclassDefaultThroughSubtrait {
    // #[() -> 7]
    function direct() public returns (uint256) {
        return uint256(A.fa(T.T));
    }

    // #[() -> 7]
    function generic() public returns (uint256) {
        return uint256(viaA(T.T));
    }

    // #[() -> 8]
    function subtrait() public returns (uint256) {
        return uint256(viaB(T.T));
    }

    // #[() -> 9]
    function nested() public returns (uint256) {
        return uint256(viaB(Wrap.Wrap(T.T)));
    }
}
