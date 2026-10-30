pragma solcore;

pragma no-patterson-condition C;

trait C<a> {}

impl<a> C<a> where a: C {}

function needsC<a>(x: a) returns () where a: C {
    return ();
}

function main(x: word) returns () {
    return needsC(x);
}
