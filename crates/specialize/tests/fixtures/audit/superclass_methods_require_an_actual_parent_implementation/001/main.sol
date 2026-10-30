pragma solcore;

//~ specialize: loader bare
//~ specialize: diagnostic missing-evidence

trait Parent<t> {
    function f(x: t) returns (word);
}
trait Child<t> where t: Parent {
    function f(x: t) returns (word);
}
impl Child<word> {
    function f(x: word) returns (word) {
        return 99;
    }
}
function run<t>(x: t) returns (word) where t: Child {
    return Parent.f(x);
}
function main() returns (word) {
    let x: word = 0;
    return run(x);
}
