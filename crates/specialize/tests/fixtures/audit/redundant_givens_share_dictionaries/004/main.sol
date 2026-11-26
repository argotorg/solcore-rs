pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: forbid instantiation-depth-exceeded
//~ specialize: forbid missing-evidence

import * from std;

// Each trait is the other's superclass, and both have conditional impls.
trait Parent<t> where t: Child {
    function parent(x: t) returns (word);
}

trait Child<t> where t: Parent {
    function child(x: t) returns (word);
}

impl Parent<uint256> {
    function parent(x: uint256) returns (word) {
        return Typedef.rep(x);
    }
}

impl Child<uint256> {
    function child(x: uint256) returns (word) {
        return Typedef.rep(x);
    }
}

enum Box<a> {
    Box(a)
}

impl<a> Parent<Box<a>> where a: Parent {
    function parent(x: Box<a>) returns (word) {
        match (x) {
            case Box(inner) {
                return Parent.parent(inner);
            }
        }
    }
}

impl<a> Child<Box<a>> where a: Child {
    function child(x: Box<a>) returns (word) {
        match (x) {
            case Box(inner) {
                return Child.child(inner);
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

function main(n: uint256) returns (word) {
    return viaChild(Box(n), n);
}
