pragma solcore;

import * from std;
import * from std.dispatch;

// Each trait is the other's superclass, and both have conditional impls.
trait Parent<t> where t: Child {
    function parent(x: t) returns (word);
}

trait Child<t> where t: Parent {
    function child(x: t) returns (word);
}

impl Parent<uint256> {
    function parent(x: uint256) returns (word) {
        return primAddWord(Typedef.rep(x), 1);
    }
}

impl Child<uint256> {
    function child(x: uint256) returns (word) {
        return primAddWord(Typedef.rep(x), 2);
    }
}

enum Box<a> {
    Box(a)
}

impl<a> Parent<Box<a>> where a: Parent {
    function parent(x: Box<a>) returns (word) {
        match (x) {
            case Box(inner) {
                return primAddWord(Parent.parent(inner), 30);
            }
        }
    }
}

impl<a> Child<Box<a>> where a: Child {
    function child(x: Box<a>) returns (word) {
        match (x) {
            case Box(inner) {
                return primAddWord(Child.child(inner), 40);
            }
        }
    }
}

function viaChild<t>(x: t, n: uint256) returns (word) where t: Child {
    if (n == 0) {
        return Parent.parent(x);
    }
    return viaParent(x, n - 1);
}

function viaParent<t>(x: t, n: uint256) returns (word) where t: Parent {
    if (n == 0) {
        return Child.child(x);
    }
    return viaChild(x, n - 1);
}

function parentOf<t>(x: t) returns (word) where t: Parent {
    return Parent.parent(x);
}

contract CyclicSuperclassRecursion {
    // #[(5, 0) -> 36]
    // #[(5, 1) -> 47]
    // #[(5, 4) -> 36]
    function cyclic(x: uint256, n: uint256) public returns (uint256) {
        return uint256(viaChild(Box.Box(x), n));
    }

    // #[(5) -> 36]
    function cyclicDictionary(x: uint256) public returns (uint256) {
        return uint256(parentOf(Box.Box(x)));
    }
}
