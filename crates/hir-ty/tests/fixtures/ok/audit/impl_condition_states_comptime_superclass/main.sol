pragma solcore;

pragma no-patterson-condition;

enum Pair<a, b> {
    Pair(a, b)
}
trait A<t> {
    function fa(x: t) returns (word);
}
trait D<t> where t: A {
    function fd(x: t) returns (word);
}
impl<x> A<Pair<comptime<x>, x>> {
    function fa(p: Pair<comptime<x>, x>) returns (word) {
        return 1;
    }
}
// The condition states the superclass of the head, qualifier included.
impl<x> D<Pair<comptime<x>, x>> where Pair<comptime<x>, x>: A {
    function fd(p: Pair<comptime<x>, x>) returns (word) {
        return 2;
    }
}
function main() returns (word) {
    return 0;
}
