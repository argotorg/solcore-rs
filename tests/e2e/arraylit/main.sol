import * from std;
import * from std.dispatch;

// Array literals, end to end.
//
// `[e1,...,en]` builds a memory array. Assigning one to a storage array field is
// Solidity's memory -> storage copy: it resizes the field and clears the
// abandoned tail, so shrinking must not leave old elements reachable.
// #[scenario("arraylit")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x286a5e5e0000000000000000000000000000000000000000000000000000000000000000, label = "memAt(0) - element 0 is the first element, not the length word") -> success(0x000000000000000000000000000000000000000000000000000000000000000b)]
// #[raw(0x286a5e5e0000000000000000000000000000000000000000000000000000000000000002, label = "memAt(2) - last element") -> success(0x0000000000000000000000000000000000000000000000000000000000000021)]
// #[raw(0x286a5e5e0000000000000000000000000000000000000000000000000000000000000003, label = "memAt(3) - out of bounds on a 3 element literal") -> revert(0xb4120f14)]
// #[raw(0xf6179f51, label = "memSum() - 1+2+3+4") -> success(0x000000000000000000000000000000000000000000000000000000000000000a)]
// #[raw(0x4ce2d543, label = "nested() - [[1,2],[3,4]][1][0]") -> success(0x0000000000000000000000000000000000000000000000000000000000000003)]
// #[raw(0x56d88e27, label = "len() - storage array initially empty") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x0960bf64, label = "setThree() - xs = [10,20,30]") -> success(0x)]
// #[raw(0x56d88e27, label = "len() - now 3") -> success(0x0000000000000000000000000000000000000000000000000000000000000003)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000000, label = "get(0) - 10") -> success(0x000000000000000000000000000000000000000000000000000000000000000a)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000002, label = "get(2) - 30") -> success(0x000000000000000000000000000000000000000000000000000000000000001e)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000003, label = "get(3) - out of bounds") -> revert(0xb4120f14)]
// #[raw(0x61125608, label = "setFive() - xs = [1,2,3,4,5], overwriting the 3 element array") -> success(0x)]
// #[raw(0x56d88e27, label = "len() - now 5") -> success(0x0000000000000000000000000000000000000000000000000000000000000005)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000004, label = "get(4) - 5") -> success(0x0000000000000000000000000000000000000000000000000000000000000005)]
// #[raw(0x65ee9559, label = "setTwo() - xs = [7,8]; shrinks from 5 and must clear the tail") -> success(0x)]
// #[raw(0x56d88e27, label = "len() - now 2") -> success(0x0000000000000000000000000000000000000000000000000000000000000002)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000000, label = "get(0) - 7") -> success(0x0000000000000000000000000000000000000000000000000000000000000007)]
// #[raw(0x28531f060000000000000000000000000000000000000000000000000000000000000005, label = "grow(5) - regrow without writing elements") -> success(0x)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000002, label = "get(2) - THE tail-clearing test: must be 0, not the 3 left by setFive") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000004, label = "get(4) - must be 0, not the 5 left by setFive") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x52efea6e, label = "clear() - xs = []") -> success(0x)]
// #[raw(0x56d88e27, label = "len() - now 0") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x9507d39a0000000000000000000000000000000000000000000000000000000000000000, label = "get(0) - out of bounds on the cleared array") -> revert(0xb4120f14)]
contract ArrayLit {
    xs: array<uint256>;

    constructor() {}

    // --- memory literals ---

    // Reads back an element of a memory literal. Element 0 must be the first
    // element, not the length word stored ahead of it.
    function memAt(i: uint256) public returns (uint256) {
        let m: memory<DynArray<uint256>> = [11, 22, 33];
        return m[i];
    }

    function memSum() public returns (uint256) {
        let m: memory<DynArray<uint256>> = [1, 2, 3, 4];
        let acc: uint256 = uint256(0);
        let i: uint256;
        for (i = uint256(0); i < uint256(4); i = i + uint256(1)) {
            acc = acc + m[i];
        }
        return acc;
    }

    // Nested literal: the element type is itself a memory array.
    function nested() public returns (uint256) {
        let g: memory<DynArray<memory<DynArray<uint256>>>> = [[1, 2], [3, 4]];
        let row: memory<DynArray<uint256>> = g[uint256(1)];
        return row[uint256(0)];
    }

    // --- storage literals ---

    function setThree() public {
        xs = [10, 20, 30];
    }

    function setFive() public {
        xs = [1, 2, 3, 4, 5];
    }

    function setTwo() public {
        xs = [7, 8];
    }

    function clear() public {
        xs = [];
    }

    function len() public returns (uint256) {
        return Length.length(xs);
    }

    function get(i: uint256) public returns (uint256) {
        return xs[i];
    }

    // Grow the array back without writing elements. Anything the shrink abandoned
    // must read as zero, not as the old value.
    function grow(n: uint256) public {
        Array.setLength(xs, n);
    }
}
