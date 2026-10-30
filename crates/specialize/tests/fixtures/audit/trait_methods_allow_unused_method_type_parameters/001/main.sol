pragma solcore;

//~ specialize: loader bare

trait C<t> {
    function f<unused, u>(x: t, y: u) returns (u);
}
impl C<word> {
    function f<unused, u>(x: word, y: u) returns (u) {
        return y;
    }
}
function main(x: word, y: word) returns (word) {
    return C.f(x, y);
}
