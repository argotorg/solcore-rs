pragma solcore;

pragma no-patterson-condition Loop;

trait Loop<a> {}

impl<a> Loop<a> where a: Loop {}

function needsLoop<a>(x: a) where a: Loop {
    return ();
}

function main() {
    return needsLoop(0);
}
