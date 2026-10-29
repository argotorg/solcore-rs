pragma solcore;

//~ specialize: loader bare
//~ specialize: return 7

enum Box<a> {
    Box(a)
}
trait C<t> {
    function f(x: t) returns (word);
}
impl<a> C<Box<a>> {
    function f(x: Box<a>) returns (word) {
        return 7;
    }
}
function indirect<t>(x: t) returns (word) where t: C {
    return C.f(x);
}
function main() returns (word) {
    let x: comptime<Box<comptime<word>>> = Box(3);
    return C.f(x);
}
