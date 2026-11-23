pragma solcore;

import * from std;

export { Q, Box(*), B, sizeViaB, boxed };

trait Q<t> {}

enum Box<a> {
    Box(a)
}

default impl<a> StorageSize<Box<a>> where a: Q {
    function size(x: Proxy<Box<a>>) returns (word) {
        return 2;
    }
}

trait B<t> where t: StorageSize {
    function fb(x: t) returns (word);
}

// The superclass `Box<a>: StorageSize` is selected where `a` is known.
impl<a> B<Box<a>> {
    function fb(x: Box<a>) returns (word) {
        return 10;
    }
}

function sizeViaB<v>(x: v) returns (word) where v: B {
    let p: Proxy<v> = Proxy;
    return StorageSize.size(p);
}

// This module has no impl of `Q`. The caller's `t: Q` is what selects the
// conditional default for the superclass of `Box<t>: B`.
function boxed<t>(x: t) returns (word) where t: Q {
    return sizeViaB(Box(x));
}
