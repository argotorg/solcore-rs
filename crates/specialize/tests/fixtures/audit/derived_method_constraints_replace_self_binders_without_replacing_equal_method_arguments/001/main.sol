pragma solcore;

//~ specialize: loader std
//~ specialize: frontend clean
//~ specialize: return 106

import { Generic } from std.Generic;
trait P<t> {
    function p(x: t) returns (word);
}
trait C<t> {
    function f<u>(x: t, y: u) returns (word) where t: P, u: P;
}
impl P<word> {
    function p(x: word) returns (word) {
        return x;
    }
}
impl C<word> {
    function f<u>(x: word, y: u) returns (word) where word: P, u: P {
        return primAddWord(P.p(x), P.p(y));
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
function main() returns (word) {
    return C.f(Box(7), Box(3));
}
