pragma solcore;

// `c[i] = v` on a user-defined type stores through its `LValueIdxAccess`
// instance, the same class the std arrays implement (#172).
import * from std;
import * from std.dispatch;

// A memory array behind a user-defined type.
enum Wrap {
    Wrap(uint256[])
}

impl<i> LValueIdxAccess<(Wrap, i), memref<uint256>> where i: Typedef<word> {
    function lookup(xi: (Wrap, i)) returns (memref<uint256>) {
        match (xi) {
            case (Wrap(inner), j) {
                return lidx(inner, uint256(Typedef.rep(j)));
            }
        }
    }
}

impl<i> RValueIdxAccess<(Wrap, i), uint256> where i: Typedef<word> {
    function lookup(xi: (Wrap, i)) returns (uint256) {
        match (xi) {
            case (Wrap(inner), j) {
                return inner[uint256(Typedef.rep(j))];
            }
        }
    }
}

// A collection that can only be written, through a reference type of its own
// that stores twice the value.
enum Doubling {
    Doubling(uint256[])
}

enum DoublingRef {
    DoublingRef(memref<uint256>)
}

impl<i> LValueIdxAccess<(Doubling, i), DoublingRef> where i: Typedef<word> {
    function lookup(xi: (Doubling, i)) returns (DoublingRef) {
        match (xi) {
            case (Doubling(inner), j) {
                return DoublingRef(lidx(inner, uint256(Typedef.rep(j))));
            }
        }
    }
}

impl CanStore<DoublingRef, uint256> {
    function store(r: DoublingRef, v: uint256) returns (()) {
        match (r) {
            case DoublingRef(cell) {
                assignRef(cell, v * 2);
            }
        }
    }
    function load(r: DoublingRef) returns (uint256) {
        match (r) {
            case DoublingRef(cell) {
                return CanStore.load(cell);
            }
        }
    }
}

// `t[0]` records calls as decimal digits: each call appends its tag.
function note(t: uint256[], tag: uint256) {
    let trace: uint256 = t[0];
    t[0] = trace * 10 + tag;
}

function same(t: uint256[], tag: uint256, w: Wrap) returns (Wrap) {
    note(t, tag);
    return w;
}

function at(t: uint256[], tag: uint256, i: uint256) returns (uint256) {
    note(t, tag);
    return i;
}

function value(t: uint256[], tag: uint256, v: uint256) returns (uint256) {
    note(t, tag);
    return v;
}

contract UserCollectionIndexAssign {
    // #[() -> 7]
    function assign() public returns (uint256) {
        let m: uint256[] = new uint256[](2);
        m[1] = 20;
        let w: Wrap = Wrap(m);
        let i: uint256 = 1;
        w[i] = 7;
        return m[1];
    }

    // #[() -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    function assignOutOfBounds() public returns (uint256) {
        let m: uint256[] = new uint256[](2);
        let w: Wrap = Wrap(m);
        w[2] = 7;
        return m[1];
    }

    // #[() -> 25]
    function compound() public returns (uint256) {
        let m: uint256[] = new uint256[](2);
        m[1] = 20;
        let w: Wrap = Wrap(m);
        w[1] += 5;
        return m[1];
    }

    // The collection and then the index are evaluated once, before the value.
    // #[() -> (123, 15, 20)]
    function compoundOrder() public returns (uint256, uint256, uint256) {
        let t: uint256[] = new uint256[](1);
        let m: uint256[] = [uint256(10), uint256(20)];
        let w: Wrap = Wrap(m);
        same(t, 1, w)[at(t, 2, 0)] += value(t, 3, 5);
        return (t[0], m[0], m[1]);
    }

    // #[() -> 8]
    function customReference() public returns (uint256) {
        let m: uint256[] = new uint256[](1);
        let d: Doubling = Doubling(m);
        d[0] = 4;
        return m[0];
    }
}
