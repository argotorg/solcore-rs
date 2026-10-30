pragma solcore;

trait Parent<t> {
    function f(x: t) returns (word);
}
trait Child<t> where t: Parent {
    function f(x: t) returns (word);
}
trait Sibling<t> where t: Parent {}
impl Sibling<word> {}
impl Child<word> {
    function f(x: word) returns (word) {
        return Parent.f(x);
    }
}
