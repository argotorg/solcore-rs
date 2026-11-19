pragma solcore;

import * from std;
import * from std.dispatch;
import * from lib;

trait Q<t> {}

enum Foo {
    Foo
}

impl Q<Foo> {}

enum Box<a> {
    Box(a)
}

// Takes precedence over std's `default impl<self> StorageSize<self>` (size 1)
// wherever `a: Q` holds.
default impl<a> StorageSize<Box<a>> where a: Q {
    function size(x: Proxy<Box<a>>) returns (word) {
        return 2;
    }
}

function sizeOf<v>(x: v) returns (word) where v: StorageSize {
    let p: Proxy<v> = Proxy;
    return StorageSize.size(p);
}

// `u: Q` is not known here, so the constraint must be stated; the caller then
// resolves it for `Foo` exactly as a direct use does.
function generic<u>(x: u) returns (word) where Box<u>: StorageSize {
    return sizeOf(Box(x));
}

enum V {
    V
}

impl A<V> {
    function fa(x: V) returns (word) {
        return 2;
    }
}

contract DefaultImplWhereClause {
    // #[() -> 2]
    function direct() public returns (uint256) {
        return uint256(sizeOf(Box(Foo.Foo)));
    }

    // #[() -> 2]
    function viaGeneric() public returns (uint256) {
        return uint256(generic(Foo.Foo));
    }

    // #[() -> 2]
    function directComptime() public returns (uint256) {
        return uint256(A.fa(V.V));
    }

    // #[() -> 2]
    function viaComptime() public returns (uint256) {
        let x: comptime<V> = V.V;
        return uint256(viaCt(x));
    }
}
