pragma solcore;

//~ specialize: loader bare
//~ specialize: return 1

trait Parent<t> where t: Child {
    function f(x: t) returns (word);
}
trait Child<t> where t: Parent {
    function g(x: t) returns (word);
}
impl Parent<word> {
    function f(x: word) returns (word) {
        return 1;
    }
}
impl Child<word> {
    function g(x: word) returns (word) {
        return 2;
    }
}
function fromChild<t>(x: t) returns (word) where t: Child {
    return Parent.f(x);
}
function fromParent<t>(x: t) returns (word) where t: Parent {
    return fromChild(x);
}
function relay<t>(x: t) returns (word) where t: Child {
    return fromParent(x);
}
function main() returns (word) {
    let x: word = 0;
    return relay(x);
}
