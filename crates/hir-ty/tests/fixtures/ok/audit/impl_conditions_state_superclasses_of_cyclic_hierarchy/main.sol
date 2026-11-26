pragma solcore;

pragma no-patterson-condition;

trait K0<a, p> where a: K3<p>, a: K1, a: K4 {
    function mK0(x: a) returns (p);
}
trait K1<a> where a: K3<word>, a: K2<word>, a: K2<bool> {
    function mK1(x: a) returns (word);
}
trait K2<a, p> where a: K4, a: K0<p>, a: K0<bool> {
    function mK2(x: a) returns (p);
}
trait K3<a, p> where a: K2<p> {
    function mK3(x: a) returns (p);
}
trait K4<a> where a: K0<word>, a: K0<bool>, a: K2<bool> {
    function mK4(x: a) returns (word);
}

// Every superclass of `K1` is a condition, as in std's default impl of `Num`.
// The conditions carry the dictionaries: none of them has to be derived, which
// this hierarchy is too tangled for.
default impl<a> K1<a> where a: K3<word>, a: K2<word>, a: K2<bool> {
    function mK1(x: a) returns (word) {
        return 0;
    }
}
function main() returns (word) {
    return 0;
}
