pragma solcore;

import * from std;

trait Q<t> {}

enum Foo {
    Foo
}

impl Q<Foo> {}

enum Box<a> {
    Box(a)
}

// Takes precedence over std's `default impl<self> StorageSize<self>` wherever
// `a: Q` holds.
default impl<a> StorageSize<Box<a>> where a: Q {
    function size(x: Proxy<Box<a>>) returns (word) {
        return 2;
    }
}

function sizeOf<v>(x: v) returns (word) where v: StorageSize {
    let p: Proxy<v> = Proxy;
    return StorageSize.size(p);
}

// `u: Q` is not known here, so neither default may decide `Box<u>: StorageSize`:
// `generic(Foo.Foo)` would otherwise use std's default while
// `sizeOf(Box(Foo.Foo))` uses the one above.
function generic<u>(x: u) returns (word) {
    return sizeOf(Box(x));
}

// Stating the constraint moves the choice to the caller, which has to state it
// as well.
function stated<v>(x: v) returns (word) where Box<v>: StorageSize {
    return sizeOf(Box(x));
}

function caller<u>(x: u) returns (word) {
    return stated(x);
}

function direct() returns (word) {
    return sizeOf(Box(Foo.Foo)) + stated(Foo.Foo);
}
