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

impl A<uint256> {
    function fa(x: uint256) returns (word) {
        return Typedef.rep(x);
    }
}

impl B<uint256> {
    function fb(x: uint256) returns (word) {
        return primAddWord(Typedef.rep(x), 10);
    }
}

impl C<uint256> {
    function fc(x: uint256) returns (word) {
        return primAddWord(Typedef.rep(x), 100);
    }
}

enum Box<a> {
    Box(a)
}

impl<a> A<Box<a>> where a: A {
    function fa(x: Box<a>) returns (word) {
        match (x) {
            case Box(inner) {
                return primAddWord(A.fa(inner), 1000);
            }
        }
    }
}

// The superclass `Box<a>: A` needs `a: A`, which both conditions give.
impl<a> B<Box<a>> where a: B, a: C {
    function fb(x: Box<a>) returns (word) {
        match (x) {
            case Box(inner) {
                return primAddWord(B.fb(inner), C.fc(inner));
            }
        }
    }
}

function stated<t>(x: t) returns (word) where t: A, t: B {
    return primAddWord(A.fa(x), B.fb(x));
}

function diamond<t>(x: t) returns (word) where t: B, t: C {
    return primAddWord(A.fa(x), primAddWord(B.fb(x), C.fc(x)));
}

// Each call fixes `t` to uint256, so the recursion has a single
// specialization.
function walk<t>(x: t, n: uint256) returns (word) where t: B, t: C {
    if (n == 0) {
        return A.fa(x);
    }
    return walk(n, n - 1);
}

function same<t>(x: t, y: t) returns (bool) where t: Eq, t: Ord {
    return Eq.eq(x, y);
}

function isZero<t>(x: t) returns (bool) where t: Ord, t: Num {
    return Eq.eq(Num.fromInteger(0), x);
}

contract GivenRedundantSuperclass {
    // #[(5) -> 20]
    function statedUint(x: uint256) public returns (uint256) {
        return uint256(stated(x));
    }

    // #[(5) -> 125]
    function diamondUint(x: uint256) public returns (uint256) {
        return uint256(diamond(x));
    }

    // #[(5) -> 1125]
    function statedBox(x: uint256) public returns (uint256) {
        return uint256(stated(Box.Box(x)));
    }

    // #[(3) -> 1]
    // #[(0) -> 0]
    function walkUint(x: uint256) public returns (uint256) {
        return uint256(walk(x, x));
    }

    // #[(3, 3) -> true]
    // #[(3, 4) -> false]
    function sameUint(x: uint256, y: uint256) public returns (bool) {
        return same(x, y);
    }

    // #[(0) -> true]
    // #[(7) -> false]
    function isZeroUint(x: uint256) public returns (bool) {
        return isZero(x);
    }
}
