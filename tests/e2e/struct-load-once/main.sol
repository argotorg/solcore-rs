import * from std;
import * from std.dispatch;
import * from std.Generic;
import { sload, sstore } from std.opcodes;

struct Point {
    x: uint256;
    y: uint256;
}

impl StorageSize<Point> {
    function size(p: Proxy<Point>) returns (word) {
        return 1;
    }
}

impl CanStore<storage<Point>, Point> {
    function store(p: storage<Point>, value: Point) {}
    function load(p: storage<Point>) returns (Point) {
        let n: word = sload(200);
        sstore(200, n + 1);
        return Point(uint256(n), 20);
    }
}

contract StructLoadOnce {
    p: Point;

    // The generated read shares the receiver saved for the field update.
    // #[() -> 1]
    function compound() public returns (uint256) {
        sstore(200, 0);
        p.x += 5;
        return uint256(sload(200));
    }

    // #[() -> 1]
    function unary() public returns (uint256) {
        sstore(200, 0);
        p.x ~=;
        return uint256(sload(200));
    }

    // Explicit source reads remain independent of the saved receiver.
    // #[() -> 2]
    function plain() public returns (uint256) {
        sstore(200, 0);
        p.x = p.x + 5;
        return uint256(sload(200));
    }

    // #[() -> 2]
    function explicitOperand() public returns (uint256) {
        sstore(200, 0);
        p.x += p.y;
        return uint256(sload(200));
    }
}
