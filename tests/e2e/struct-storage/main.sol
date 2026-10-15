import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

// A Solidity-style struct used directly as a contract storage field.
//
// A `struct` is a single-constructor product, so it stores exactly like the
// `Triple` product in storage_adt_field.sol: each field occupies its own
// StorageSize-many slots (here 2 uint256 words = 2 slots), reached through the
// derived storage(Point):CanStore(Point) instance. The whole struct can be
// written and read back, destructured with `match`, or read field-by-field via
// dot notation (a whole-struct load followed by a field projection).
//
// A single field can also be *written* through dot notation: `p.x = v`
// desugars to a whole-struct read-modify-write `p = set_x(p, v)` (the setter is
// generated per field alongside the projections), reusing the same
// CanStore.store path.

struct Point {
    x: uint256;
    y: uint256;
}

// #[scenario("storage_struct")]
// constructor() asserts StorageSize(Point)==2
// #[deploy(0x) -> success]
// #[raw(0x4b5bdd8e00000000000000000000000000000000000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000004, label = "setPoint(3,4): whole-struct store -> p=(3,4)") -> success(0x)]
// #[raw(0xf680b3a7, label = "sumMatch(): match p -> 3+4 = 7") -> success(0x0000000000000000000000000000000000000000000000000000000000000007)]
// #[raw(0x5197c7aa, label = "getX(): dot read p.x -> 3") -> success(0x0000000000000000000000000000000000000000000000000000000000000003)]
// #[raw(0x0b7f1665, label = "getY(): dot read p.y -> 4") -> success(0x0000000000000000000000000000000000000000000000000000000000000004)]
// #[raw(0x4018d9aa0000000000000000000000000000000000000000000000000000000000000009, label = "setX(9): sub-field write p.x=9 (RMW) -> p=(9,4)") -> success(0x)]
// #[raw(0x5197c7aa, label = "getX() -> 9 (updated)") -> success(0x0000000000000000000000000000000000000000000000000000000000000009)]
// #[raw(0x0b7f1665, label = "getY() -> 4 (other field preserved)") -> success(0x0000000000000000000000000000000000000000000000000000000000000004)]
// #[raw(0x68d466b8000000000000000000000000000000000000000000000000000000000000000b, label = "setY(11): sub-field write p.y=11 -> p=(9,11)") -> success(0x)]
// #[raw(0xf680b3a7, label = "sumMatch(): 9+11 = 20") -> success(0x0000000000000000000000000000000000000000000000000000000000000014)]
contract C {
    p: Point;

    constructor() {
        // product: size uint256 * 2 = 2 slots
        assert(StorageSize.size(@Point) == 2);
    }

    // whole-struct write
    function setPoint(a: uint256, b: uint256) public returns (()) {
        p = Point(a, b);
    }

    // read back by destructuring with match
    function sumMatch() public returns (uint256) {
        match (p) {
            case Point(a, b) {
                return a + b;
            }
        }
    }

    // read a single field via dot access on the storage struct
    function getX() public returns (uint256) {
        return p.x;
    }

    function getY() public returns (uint256) {
        return p.y;
    }

    // write a single field via dot access (read-modify-write the whole struct)
    function setX(v: uint256) public returns (()) {
        p.x = v;
    }

    function setY(v: uint256) public returns (()) {
        p.y = v;
    }
}
