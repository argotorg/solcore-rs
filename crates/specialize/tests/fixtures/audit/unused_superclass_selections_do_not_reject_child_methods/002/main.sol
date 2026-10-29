pragma solcore;

//~ specialize: loader bare
//~ specialize: return 99

trait Parent<t> where t: Child {
    function f(x: t) returns (word);
}
trait Child<t> where t: Parent {
    function g(x: t) returns (word);
}
trait Other<t> where t: Parent {}
impl Child<word> {
    function g(x: word) returns (word) {
        return 99;
    }
}
impl Other<word> {}
function main() returns (word) {
    let x: word = 0;
    return Child.g(x);
}
