pragma solcore;

trait A<a> {
    function f(x: a) returns (word);
}
trait B<a> where a: A {}
function run<a>(x: a) returns (word) where a: A, a: B {
    return A.f(x);
}
impl A<word> {
    function f(x: word) returns (word) {
        return 7;
    }
}
impl B<word> {}
function main() returns (word) {
    let x: word = 0;
    return run(x);
}
