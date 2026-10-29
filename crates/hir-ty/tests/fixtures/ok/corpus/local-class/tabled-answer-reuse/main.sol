pragma solcore;

pragma no-patterson-condition Derived;

trait Seed<a> {}
trait Derived<a> {}

impl Seed<word> {}

impl<a> Derived<a> where a: Seed {}

function needsDerivedTwice<a>(x: a) where a: Derived, a: Derived {
    return ();
}

function main() {
    let value: word = 0;
    return needsDerivedTwice(value);
}
