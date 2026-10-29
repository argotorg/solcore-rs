pragma solcore;

trait C<a> {}
trait Sub<a> where a: C {}
enum Box<a> {
    Box(a)
}
type Layer(a) = Box<a>;
impl C<word> {}
impl<a> C<Layer<a>> where a: C {}
impl<a> Sub<Layer<a>> where a: C {}
function require<a>(x: a) returns (word) where a: C {
    return 1;
}
function main(x: Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<Layer<word>>>>>>>>>>>>>>>>>>>>>>>>) returns (word) {
    let f = require;
    return f(x);
}
