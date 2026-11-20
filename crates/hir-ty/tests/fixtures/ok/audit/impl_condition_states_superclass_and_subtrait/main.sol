pragma solcore;

pragma no-patterson-condition;

trait A<a> {
    function f(x: a) returns (word);
}
trait C<a> where a: A {}
trait D<a> where a: A {}
impl<a> D<a> where a: A, a: C {}
impl A<word> {
    function f(x: word) returns (word) {
        return 7;
    }
}
impl C<word> {}
function run<a>(x: a) returns (word) where a: D {
    return A.f(x);
}
function main() returns (word) {
    let x: word = 0;
    return run(x);
}
