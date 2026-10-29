pragma solcore;

trait C<a> {
    function f(x: a) returns (word);
}
trait D<a> {}
enum Box<a> {
    Box(a)
}
impl C<word> {
    function f(x: word) returns (word) {
        return 2;
    }
}
impl C<word> {
    function f(x: word) returns (word) {
        return 1;
    }
}
impl<a> D<Box<a>> where a: C {}
