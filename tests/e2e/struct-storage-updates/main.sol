pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

// #[scenario("struct_storage_updates")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0xd32cb0fe, label = "exercise() returns updated x=22, preserved y=7, restored array value=12, and four index evaluations") -> success(0x00000000000000000000000000000000000000000000000000000000000000160000000000000000000000000000000000000000000000000000000000000007000000000000000000000000000000000000000000000000000000000000000c0000000000000000000000000000000000000000000000000000000000000004)]
contract StructStorageUpdates {
    struct Point {
        x: uint256;
        y: uint256;
    }
    p: Point;
    xs: array<uint256>;
    ticks: uint256;

    constructor() {}

    function nextIndex() returns (uint256) {
        ticks += 1;
        return 0;
    }

    // Contract-local named products need valid qualified getter/setter and Hull
    // type names. The updates must retain the untouched second field.
    function exercise() public returns ((uint256, uint256, uint256, uint256)) {
        p = Point(12, 7);
        p.x *= 3;
        p.x /= 2;
        p.x += 4;
        p.x ~=;
        p.x ~=;
        // Each compound update evaluates its indexing expression exactly once.
        xs = [12];
        ticks = 0;
        xs[nextIndex()] *= 3;
        xs[nextIndex()] /= 3;
        xs[nextIndex()] ~=;
        xs[nextIndex()] ~=;
        // Fields nested in a tuple constructor also undergo storage desugaring.
        return (p.x, p.y, xs[uint256(0)], ticks);
    }
}
