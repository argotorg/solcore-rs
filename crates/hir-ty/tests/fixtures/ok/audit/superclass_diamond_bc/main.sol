pragma solcore;

trait A<a> {
    function f(x: a) returns (word);
}
trait B<a> where a: A {}
trait C<a> where a: A {}
trait D<a> where a: B, a: C {}
function run<a>(x: a) returns (word) where a: D {
    return A.f(x);
}
impl A<word> {
    function f(x: word) returns (word) {
        return 7;
    }
}
impl B<word> {}
impl C<word> {}
impl D<word> {}
function main() returns (word) {
    let x: word = 0;
    return run(x);
}
