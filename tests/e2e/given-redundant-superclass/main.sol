pragma solcore;

import * from std;
import * from std.dispatch;

// A predicate that two local givens entail is one dictionary. A body that
// states `t: A` next to a subtrait, or two subtraits of `A`, runs the same `A`
// impl as a direct call.
trait A<t> {
    function fa(x: t) returns (word);
}

trait B<t> where t: A {
    function fb(x: t) returns (word);
}

trait C<t> where t: A {
    function fc(x: t) returns (word);
}

enum T {
    T
}

impl A<T> {
    function fa(x: T) returns (word) {
        return 1;
    }
}

impl B<T> {
    function fb(x: T) returns (word) {
        return 10;
    }
}

impl C<T> {
    function fc(x: T) returns (word) {
        return 100;
    }
}

enum U {
    U
}

impl A<U> {
    function fa(x: U) returns (word) {
        return 2;
    }
}

impl B<U> {
    function fb(x: U) returns (word) {
        return 20;
    }
}

impl C<U> {
    function fc(x: U) returns (word) {
        return 200;
    }
}

function stated<t>(x: t) returns (word) where t: A, t: B {
    return primAddWord(A.fa(x), B.fb(x));
}

function diamond<t>(x: t) returns (word) where t: B, t: C {
    return primAddWord(A.fa(x), primAddWord(B.fb(x), C.fc(x)));
}

function same<t>(x: t, y: t) returns (bool) where t: Eq, t: Ord {
    return Eq.eq(x, y);
}

contract GivenRedundantSuperclass {
    // #[() -> 11]
    function statedT() public returns (uint256) {
        return uint256(stated(T.T));
    }

    // #[() -> 22]
    function statedU() public returns (uint256) {
        return uint256(stated(U.U));
    }

    // #[() -> 111]
    function diamondT() public returns (uint256) {
        return uint256(diamond(T.T));
    }

    // #[() -> 222]
    function diamondU() public returns (uint256) {
        return uint256(diamond(U.U));
    }

    // #[(3, 3) -> true]
    // #[(3, 4) -> false]
    function sameUint(x: uint256, y: uint256) public returns (bool) {
        return same(x, y);
    }
}
