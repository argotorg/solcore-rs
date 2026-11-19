pragma solcore;

import * from std;

trait Q<t> {}
trait R<t> {}

enum Box<a> {
    Box(a)
}

// Two defaults of one priority: where both conditions hold, `Box<_>: StorageSize`
// is ambiguous.
default impl<a> StorageSize<Box<a>> where a: Q {
    function size(x: Proxy<Box<a>>) returns (word) {
        return 2;
    }
}

default impl<a> StorageSize<Box<a>> where a: R {
    function size(x: Proxy<Box<a>>) returns (word) {
        return 3;
    }
}

function sizeOf<v>(x: v) returns (word) where v: StorageSize {
    let p: Proxy<v> = Proxy;
    return StorageSize.size(p);
}

// The second default applies through `u: R`, but `u: Q` may hold as well once
// `u` is known.
function generic<u>(x: u) returns (word) where u: R {
    return sizeOf(Box(x));
}
