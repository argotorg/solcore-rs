pragma solcore;

//~ specialize: loader bare

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
function decrement(n: word) returns (word) {
    let result: word;
    assembly {
        result := sub(n, 1)
    }
    return result;
}
function recurse<t>(x: t, n: word) returns (word) where t: Child {
    match (n) {
        case (0) {
            return Parent.f(x);
        }
        case (_) {
            return viaParent(x, decrement(n));
        }
    }
}
function viaParent<t>(x: t, n: word) returns (word) where t: Parent {
    return recurse(x, n);
}
function main(n: word) returns (word) {
    let x: word = 0;
    return recurse(x, n);
}
