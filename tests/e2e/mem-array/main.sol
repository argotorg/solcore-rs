pragma solcore;

// Memory arrays as in Solidity: `new T[](n)` allocates n zeroed elements, and
// `m[i] = v` and compound assignments update an element in place after a
// bounds check, which panics with 0x32. Ported from the Haskell
// `dispatch/mem_array` regression.
import * from std;
import * from std.dispatch;

contract MemArray {
    // #[(5) -> 5]
    function newLen(n: uint256) public returns (uint256) {
        let m: uint256[] = new uint256[](n);
        return Length.length(m);
    }

    // #[(3, 1, 99) -> 99]
    // #[(3, 0, 7) -> 7]
    // #[(2, 5, 1) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000032)]
    function setGet(n: uint256, i: uint256, v: uint256) public returns (uint256) {
        let m: uint256[] = new uint256[](n);
        m[i] = v;
        return m[i];
    }

    // #[(4, 2) -> 0]
    function defaultZero(n: uint256, i: uint256) public returns (uint256) {
        let m: uint256[] = new uint256[](n);
        return m[i];
    }

    // An oversized length would make (n + 1) * 32 wrap.
    // #[(0x800000000000000000000000000000000000000000000000000000000000000) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000041)]
    function oversized(n: uint256) public returns (uint256) {
        let m: uint256[] = new uint256[](n);
        return Length.length(m);
    }

    // #[(4) -> 30]
    function fillAndSum(n: uint256) public returns (uint256) {
        let m: uint256[] = new uint256[](n);
        for (let i: uint256 = 0; i < n; i += 1) {
            m[i] = i * 5;
        }
        let total: uint256 = 0;
        for (let i: uint256 = 0; i < n; i += 1) {
            total += m[i];
        }
        return total;
    }

    // #[(10) -> 13]
    function compound(v: uint256) public returns (uint256) {
        let m: uint256[] = [v, uint256(1)];
        m[0] += 3;
        return m[0];
    }

    // Each element of `new T[][](n)` starts as an empty array, not a null
    // pointer, and can be replaced by another array.
    // #[(3) -> 0]
    function nestedEmpty(n: uint256) public returns (uint256) {
        let rows: uint256[][] = new uint256[][](n);
        return Length.length(rows[n - 1]);
    }

    // #[(4, 9) -> 9]
    function nestedReplace(n: uint256, v: uint256) public returns (uint256) {
        let rows: uint256[][] = new uint256[][](2);
        let row: uint256[] = new uint256[](n);
        row[n - 1] = v;
        rows[1] = row;
        return rows[1][n - 1] + Length.length(rows[0]);
    }

    // Literal arrays are writable too, and a write through one name is
    // visible through every alias of the same array.
    // #[(8) -> 8]
    function alias(v: uint256) public returns (uint256) {
        let m: uint256[] = [uint256(1), uint256(2)];
        let n: uint256[] = m;
        n[1] = v;
        return m[1];
    }
}
