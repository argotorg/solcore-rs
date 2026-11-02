pragma solcore;

// Array slices as in Solidity: `a[start:end]`, `a[start:]`, `a[:end]` and
// `a[:]` are read-only views whose `length` and `s[i]` delegate to the base.
// Slices work over memory and storage arrays, re-slice, and copy back into a
// fresh memory array with `toArray`. A bad bound or index panics with 0x32.
// Ported from the Haskell `dispatch/array_slice` and `spec/138arrayslice`.
import * from std;
import * from std.dispatch;

contract ArraySlice {
    xs: uint256[];

    // [10, 11, 12, 13, 14]
    function build() returns (uint256[]) {
        let a: uint256[] = new uint256[](5);
        for (let i: uint256 = 0; i < a.length; i += 1) {
            a[i] = i + 10;
        }
        return a;
    }

    // #[() -> 3]
    function rangeLen() public returns (uint256) {
        return build()[1:4].length;
    }

    // #[(0) -> 11]
    // #[(2) -> 13]
    // #[(3) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    function rangeGet(i: uint256) public returns (uint256) {
        return build()[1:4][i];
    }

    // #[(2) -> 3]
    // #[(5) -> 0]
    // #[(6) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    function fromLen(start: uint256) public returns (uint256) {
        return build()[start:].length;
    }

    // #[(3) -> 3]
    // #[(6) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    function toLen(end: uint256) public returns (uint256) {
        return build()[:end].length;
    }

    // #[() -> 5]
    function allLen() public returns (uint256) {
        return build()[:].length;
    }

    // [11, 12, 13][1:] is [12, 13].
    // #[(0) -> 12]
    // #[(1) -> 13]
    // #[(2) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    function reslice(i: uint256) public returns (uint256) {
        return build()[1:4][1:][i];
    }

    // #[(4, 2) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    // #[(0, 6) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    // #[(2, 2) -> 0]
    function bounds(start: uint256, end: uint256) public returns (uint256) {
        return build()[start:end].length;
    }

    // #[(0) -> 11]
    // #[(2) -> 13]
    // #[(3) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    function materialise(i: uint256) public returns (uint256) {
        let b: uint256[] = toArray(build()[1:4]);
        return b[i];
    }

    // The view reads the base, so later writes to the base are visible.
    // #[() -> 99]
    function aliasing() public returns (uint256) {
        let a = build();
        let s = a[2:];
        a[3] = 99;
        return s[1];
    }

    // 5 + 2 + 2 + 3 + 2 lengths, plus mid[0] = 11 and inner[1] = 13.
    // #[() -> 38]
    function spec() public returns (uint256) {
        let a = build();
        let full = a[:];
        let front = a[:2];
        let back = a[3:];
        let mid = a[1:4];
        let inner = mid[1:];
        return full.length + front.length + back.length + mid.length + inner.length + mid[0] + inner[1];
    }

    // A ternary bound consumes its own colon.
    // #[(true) -> 4]
    // #[(false) -> 3]
    function ternaryBound(c: bool) public returns (uint256) {
        return build()[c ? uint256(1) : uint256(2):].length;
    }

    // #[(0) -> 6]
    // #[(1) -> 7]
    // #[(2) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    function storageSlice(i: uint256) public returns (uint256) {
        xs.setLength(uint256(0));
        xs.push(uint256(5));
        xs.push(uint256(6));
        xs.push(uint256(7));
        return xs[1:][i];
    }

    // #[() -> 2]
    function storageSliceLength() public returns (uint256) {
        xs.setLength(uint256(0));
        xs.push(uint256(5));
        xs.push(uint256(6));
        return xs[:].length;
    }
}
