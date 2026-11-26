pragma solcore;

import * from std;
import * from std.dispatch;

// Issue #170: a subtrait call in a free function, with a recursive user impl
// of the std superclass, made the backend's desugar inference run forever.

enum Foo {
    Foo
}

enum Wrap<a> {
    Wrap(a)
}

impl<a> StorageSize<Wrap<a>> where a: StorageSize {
    function size(x: Proxy<Wrap<a>>) returns (word) {
        let p: Proxy<a> = Proxy;
        return primAddWord(StorageSize.size(p), 100);
    }
}

trait R<t> where t: StorageSize {
    function r(x: t) returns (word);
}

impl<a> R<Wrap<a>> where a: StorageSize {
    function r(x: Wrap<a>) returns (word) {
        return 7;
    }
}

function site() returns (word) {
    return R.r(Wrap(Foo.Foo));
}

function wrappedSize() returns (word) {
    let p: Proxy<Wrap<Wrap<Foo>>> = Proxy;
    return StorageSize.size(p);
}

contract C {
    // #[() -> 7]
    function run() public returns (uint256) {
        return uint256(site());
    }

    // #[() -> 201]
    function size() public returns (uint256) {
        return uint256(wrappedSize());
    }
}
