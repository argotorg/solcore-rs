pragma solcore;

import * from std;

enum Box<a> {
    Box(a)
}

enum Wrap<a> {
    Wrap(a)
}

// Takes precedence over std's `default impl<self> StorageSize<self>` for every
// `Box<Wrap<_>>`.
default impl<a> StorageSize<Box<Wrap<a>>> {
    function size(x: Proxy<Box<Wrap<a>>>) returns (word) {
        return 2;
    }
}

function sizeOf<v>(x: v) returns (word) where v: StorageSize {
    let p: Proxy<v> = Proxy;
    return StorageSize.size(p);
}

// Whether `Box<u>` is a `Box<Wrap<_>>` depends on `u`.
function generic<u>(x: u) returns (word) {
    return sizeOf(Box(x));
}
