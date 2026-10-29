pragma solcore;

pragma no-patterson-condition;
import * from base;
impl Child<word> {
    function f(x: word) returns (word) {
        return 99;
    }
}
impl Silent<word> {}
impl Grandchild<word> {}
impl<t> UsesChild<Wrapped<t>> where t: Child {
    function run(w: Wrapped<t>) returns (word) {
        match (w) {
            case (Wrapped(x)) {
                return Parent.f(x);
            }
        }
    }
}

impl Direct<word> where word: Parent {
    function call(x: word) returns (word) {
        return Parent.f(x);
    }
}

function decrement(n: word) returns (word) {
    let result: word;
    assembly {
        result := sub(n, 1)
    }
    return result;
}
impl ImplicitChild<word> {
    function f(x: word) returns (word) {
        return ImplicitParent.f(x);
    }
    function g(x: word) returns (word) {
        match (x) {
            case (0) {
                return ImplicitParent.f(x);
            }
            case (_) {
                return ImplicitChild.g(decrement(x));
            }
        }
    }
}
impl ImplicitChild<ImplicitWrapped<word>> {
    function f(x: ImplicitWrapped<word>) returns (word) {
        return ImplicitParent.f(x);
    }
    function g(x: ImplicitWrapped<word>) returns (word) {
        return ImplicitParent.f(x);
    }
}
