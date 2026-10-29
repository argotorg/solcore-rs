pragma solcore;

trait A<a> where a: B {}
trait B<a> where a: A {}
trait C<a> {}

function needsC<a>(x: a) where a: C {
    return ();
}

function cannotGetC<a>(x: a) where a: A {
    return needsC(x);
}

function main() {
    return ();
}
