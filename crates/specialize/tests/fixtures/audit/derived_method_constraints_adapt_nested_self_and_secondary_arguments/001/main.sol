pragma solcore;

//~ specialize: loader std
//~ specialize: frontend clean
//~ specialize: return 7

import { Generic } from std.Generic;
trait P<t, r> {
    function p(x: t) returns (word);
}
trait C<t> {
    function f<u, r>(x: t, y: u) returns (word) where(t, u): P<(t, r)>;
}
impl P<(word, word), (word, bool)> {
    function p(x: (word, word)) returns (word) {
        return 7;
    }
}
impl C<word> {
    function f<u, r>(x: word, y: u) returns (word) where(word, u): P<(word, r)> {
        return P.p((x, y));
    }
}
#[derive(C)]
enum Box {
    Box(word)
}
impl P<(Box, word), (Box, bool)> {
    function p(x: (Box, word)) returns (word) {
        return 99;
    }
}
function main() returns (word) {
    let value: word = 2;
    return C.f(Box(1), value);
}
