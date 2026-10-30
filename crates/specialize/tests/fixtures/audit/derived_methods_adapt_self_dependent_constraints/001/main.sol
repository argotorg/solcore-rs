pragma solcore;

//~ specialize: loader std
//~ specialize: frontend clean

import { Generic } from std.Generic;
trait P<t> {
    function p(x: t) returns (word);
}
trait C<t> {
    function f(x: t) returns (word) where t: P;
}
impl P<word> {
    function p(x: word) returns (word) {
        return x;
    }
}
impl C<word> {
    function f(x: word) returns (word) where word: P {
        return P.p(x);
    }
}
#[derive(C)]
enum Box {
    Box(word)
}
impl P<Box> {
    function p(x: Box) returns (word) {
        return 99;
    }
}
function apply(f: function (Box) returns (word), x: Box) returns (word) {
    return f(x);
}
function main() returns (word) {
    return C.f(Box(7));
}
