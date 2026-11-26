pragma solcore;

trait A<t> {
    function fa(x: t) returns (word);
}
trait B<t> where t: A {
    function fb(x: t) returns (word);
}
enum Box<a> {
    Box(a)
}
impl<a> A<Box<a>> {
    function fa(x: Box<a>) returns (word) {
        return 7;
    }
}
impl<a> B<Box<a>> {
    function fb(x: Box<a>) returns (word) {
        return 8;
    }
}
// The assumptions carry the qualifier of the parameter type. Each is the
// local dictionary of its call, ahead of the impl for `Box`.
function viaSubtrait<t>(x: Box<comptime<t>>) returns (word) where Box<comptime<t>>: B {
    return A.fa(x);
}
function direct<t>(x: Box<comptime<t>>) returns (word) where Box<comptime<t>>: A {
    return A.fa(x);
}
function main() returns (word) {
    return 0;
}
