pragma solcore;

import * from std;
import * from std.dispatch;

trait Q<t> {}

enum Foo {
    Foo
}

impl Q<Foo> {}

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

impl<a> B<Box<a>> {
    function fb(x: Box<a>) returns (word) {
        return 10;
    }
}

function pick<v>(x: v) returns (word) where v: StorageSize, v: B {
    let p: Proxy<v> = Proxy;
    return StorageSize.size(p);
}

// `site` selects std's `StorageSize` default for `Box<u>`, while the `B` impl
// selects the conditional default once `u` is `Foo`.
function site<u>(x: u) returns (word) {
    return pick(Box(x));
}

contract K {
    function main() public returns (uint256) {
        return uint256(site(Foo.Foo));
    }
}
