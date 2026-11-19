pragma solcore;

// `m[i] op= v` on a memory array evaluates `m` and `i` once, then checks the
// bounds, reads the element, evaluates `v` and writes the element it read, in
// the same order as a storage element. An explicitly written `m[i] = m[i] op v`
// still evaluates both sides (#159).
import * from std;
import * from std.dispatch;

// `t[0]` records calls as decimal digits: each call appends its tag.
function note(t: uint256[], tag: uint256) {
    let trace: uint256 = t[0];
    t[0] = trace * 10 + tag;
}

function at(t: uint256[], tag: uint256, i: uint256) returns (uint256) {
    note(t, tag);
    return i;
}

function value(t: uint256[], tag: uint256, v: uint256) returns (uint256) {
    note(t, tag);
    return v;
}

function same(t: uint256[], tag: uint256, m: uint256[]) returns (uint256[]) {
    note(t, tag);
    return m;
}

function sameRows(t: uint256[], tag: uint256, m: uint256[][]) returns (uint256[][]) {
    note(t, tag);
    return m;
}

// Returns the next index on each call.
function bump(c: uint256[]) returns (uint256) {
    let i: uint256 = c[0];
    c[0] = i + 1;
    return i;
}

// Overwrites `m[0]` and returns 1.
function clobber(m: uint256[]) returns (uint256) {
    m[0] = 100;
    return 1;
}

contract MemArrayCompoundOnce {
    stored: mapping(uint256 => uint256);

    // Evaluating `bump(c)` twice would read `m[1]` and write `m[0]`.
    // #[() -> 15]
    function issueExample() public returns (uint256) {
        let c: uint256[] = new uint256[](1);
        let m: uint256[] = new uint256[](2);
        m[0] = 10;
        m[1] = 20;
        m[bump(c)] += 5;
        return m[0];
    }

    // #[() -> (1, 15, 20)]
    function base() public returns (uint256, uint256, uint256) {
        let t: uint256[] = new uint256[](1);
        let m: uint256[] = [uint256(10), uint256(20)];
        same(t, 1, m)[0] += 5;
        return (t[0], m[0], m[1]);
    }

    // #[() -> (123, 10, 25)]
    function order() public returns (uint256, uint256, uint256) {
        let t: uint256[] = new uint256[](1);
        let m: uint256[] = [uint256(10), uint256(20)];
        same(t, 1, m)[at(t, 2, 1)] += value(t, 3, 5);
        return (t[0], m[0], m[1]);
    }

    // The storage path has the same order; the contract field has no effect.
    // #[() -> (23, 5)]
    function storageOrder() public returns (uint256, uint256) {
        let t: uint256[] = new uint256[](1);
        stored[at(t, 2, 1)] += value(t, 3, 5);
        return (t[0], stored[1]);
    }

    // The element is read before the right-hand side runs.
    // #[() -> 11]
    function readBeforeRhs() public returns (uint256) {
        let m: uint256[] = [uint256(10), uint256(20)];
        m[0] += clobber(m);
        return m[0];
    }

    // #[() -> (12345, 13, 7, 30, 3, 1)]
    function arithmetic() public returns (uint256, uint256, uint256, uint256, uint256, uint256) {
        let t: uint256[] = new uint256[](1);
        let m: uint256[] = new uint256[](5);
        for (let i: uint256 = 0; i < 5; i = i + 1) {
            m[i] = 10;
        }
        m[at(t, 1, 0)] += 3;
        m[at(t, 2, 1)] -= 3;
        m[at(t, 3, 2)] *= 3;
        m[at(t, 4, 3)] /= 3;
        m[at(t, 5, 4)] %= 3;
        return (t[0], m[0], m[1], m[2], m[3], m[4]);
    }

    // #[() -> (123456, 9, 2, 11, 80, 5, 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff5)]
    function bitwise() public returns (uint256, uint256, uint256, uint256, uint256, uint256, uint256) {
        let t: uint256[] = new uint256[](1);
        let m: uint256[] = new uint256[](6);
        for (let i: uint256 = 0; i < 6; i = i + 1) {
            m[i] = 10;
        }
        m[at(t, 1, 0)] ^= 3;
        m[at(t, 2, 1)] &= 3;
        m[at(t, 3, 2)] |= 3;
        m[at(t, 4, 3)] <<= 3;
        m[at(t, 5, 4)] >>= 1;
        m[at(t, 6, 5)] ~=;
        return (t[0], m[0], m[1], m[2], m[3], m[4], m[5]);
    }

    // #[() -> (1234, 13, 1)]
    function nested() public returns (uint256, uint256, uint256) {
        let t: uint256[] = new uint256[](1);
        let m: uint256[][] = new uint256[][](2);
        m[0] = [uint256(1), uint256(2)];
        m[1] = [uint256(3), uint256(4)];
        sameRows(t, 1, m)[at(t, 2, 1)][at(t, 3, 0)] += value(t, 4, 10);
        return (t[0], m[1][0], m[0][0]);
    }

    // A read written on the right-hand side is evaluated on its own.
    // #[() -> (23, 30)]
    function sameElementOnRhs() public returns (uint256, uint256) {
        let t: uint256[] = new uint256[](1);
        let m: uint256[] = [uint256(10), uint256(20)];
        m[at(t, 2, 0)] += m[at(t, 3, 1)];
        return (t[0], m[0]);
    }

    // An explicit read of the same place evaluates it again.
    // #[() -> (22, 21)]
    function explicitRead() public returns (uint256, uint256) {
        let t: uint256[] = new uint256[](1);
        let m: uint256[] = [uint256(10), uint256(20)];
        m[at(t, 2, 0)] = m[at(t, 2, 1)] + 1;
        return (t[0], m[0]);
    }

    // #[() -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    function outOfBounds() public returns (uint256) {
        let t: uint256[] = new uint256[](1);
        let m: uint256[] = [uint256(10), uint256(20)];
        m[at(t, 2, 2)] += 1;
        return t[0];
    }
}
