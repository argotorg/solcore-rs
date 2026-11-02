pragma solcore;

import { Generic } from std.Generic;
import * from base;
export { Wrapped(*), Derived(*), ImplicitWrapped(*), decrement };
enum Wrapped<t> {
    Wrapped(t)
}
impl<t> UsesChild<Wrapped<t>> where t: Child {
    function run(w: Wrapped<t>) returns (word) {
        match (w) {
            case (Wrapped(x)) {
                return Parent.f(x);
            }
        }
    }
}

#[derive(Child)]
enum Derived<t> {
    Derived(t)
}
impl<t> Parent<Derived<t>> where t: Parent {
    function f(x: Derived<t>) returns (word) {
        match (x) {
            case (Derived(y)) {
                return Parent.f(y);
            }
        }
    }
}

function decrement(n: word) returns (word) {
    let result: word;
    assembly {
        result := sub(n, 1)
    }
    return result;
}
#[derive(ImplicitParent)]
enum ImplicitWrapped<t> {
    ImplicitWrapped(t)
}
impl<t> ImplicitChild<ImplicitWrapped<t>> {
    function f(x: ImplicitWrapped<t>) returns (word) {
        return ImplicitParent.f(x);
    }
    function g(x: ImplicitWrapped<t>) returns (word) {
        return ImplicitParent.f(x);
    }
}
