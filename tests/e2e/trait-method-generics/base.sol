pragma solcore;

import { Generic } from std.Generic;
export { Local, Pick, Parent, Child, Derived(*), throughChild, apply, applyOne };
trait Local<t> {
    function value(x: t) returns (word);
}
trait Pick<t, r> {
    function pick(x: t) returns (word);
}
trait Parent<t> {
    function choose<u, r>(x: t, y: u) returns (word) where u: Pick<r>;
    function identity<u>(x: t, y: u) returns (u);
    function local<u>(x: t, y: u) returns (word) where u: Local;
}
trait Child<t> where t: Parent {}
#[derive(Parent)]
enum Derived {
    Derived(word)
}
function decrement(n: word) returns (word) {
    let result: word;
    assembly {
        result := sub(n, 1)
    }
    return result;
}
impl Parent<word> {
    function choose<u, r>(x: word, y: u) returns (word) where u: Pick<r> {
        match (x) {
            case (0) {
                return Pick.pick(y);
            }
            case (_) {
                return Parent.choose(decrement(x), y);
            }
        }
    }
    function local<u>(x: word, y: u) returns (word) where u: Local {
        assembly {
            mstore(0, x)
        }
        return Local.value(y);
    }
    function identity<u>(x: word, y: u) returns (u) {
        assembly {
            mstore(0, x)
        }
        return y;
    }
}
impl Child<word> {}
function throughChild<t, u, r>(x: t, y: u) returns (word) where t: Child, u: Pick<r> {
    return Parent.choose(x, y);
}
function apply(f: function (word, word) returns (word), x: word, y: word) returns (word) {
    return f(x, y);
}
function applyOne(f: function (word) returns (word), x: word) returns (word) {
    return f(x);
}
