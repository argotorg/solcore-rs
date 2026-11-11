pragma solcore;

trait Parent<t> {
    function f(x: t) returns (word);
}
trait Child<t> where t: Parent {
    function g(x: t) returns (word);
}
trait Sibling<t> where t: Parent {}
impl Parent<word> {
    function f(x: word) returns (word) {
        return 1;
    }
}
impl Sibling<word> {}
impl Child<word> {
    function g(x: word) returns (word) {
        return Parent.f(x);
    }
}
