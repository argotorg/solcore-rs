pragma solcore;

trait A<a> where a: B {}
trait B<a> where a: A {}

function needsB<a>(x: a) where a: B {
    return ();
}

function usesSuperCycle<a>(x: a) where a: A {
    return needsB(x);
}

function main() {
    return ();
}
