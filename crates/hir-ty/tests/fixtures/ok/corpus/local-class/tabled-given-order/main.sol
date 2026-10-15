pragma no-patterson-condition C;

trait A<a> {}
trait B<a> {}
trait C<a> {}

impl<a> C<a> where a: A, a: B {}

function needsC<a>(x: a) where a: C {
    return ();
}

function fromAB<a>(x: a) where a: A, a: B {
    return needsC(x);
}

function fromBA<a>(x: a) where a: B, a: A {
    return needsC(x);
}

function main() {
    return ();
}
