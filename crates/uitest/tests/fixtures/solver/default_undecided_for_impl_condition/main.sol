pragma solcore;

pragma no-patterson-condition Outer;
import * from std;

trait Q<t> {}

enum Box<a> {
    Box(a)
}

default impl<a> StorageSize<Box<a>> where a: Q {
    function size(x: Proxy<Box<a>>) returns (word) {
        return 2;
    }
}

trait Outer<t> {
    function outer(x: Proxy<t>) returns (word);
}

impl<a> Outer<Box<a>> where Box<a>: StorageSize {
    function outer(x: Proxy<Box<a>>) returns (word) {
        let p: Proxy<Box<a>> = Proxy;
        return StorageSize.size(p);
    }
}

function outerOf<v>(x: v) returns (word) where v: Outer {
    let p: Proxy<v> = Proxy;
    return Outer.outer(p);
}

// The impl's condition `Box<u>: StorageSize` is the goal left undecided.
function generic<u>(x: u) returns (word) {
    return outerOf(Box(x));
}
