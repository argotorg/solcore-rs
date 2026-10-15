import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

struct Point {
    x: uint256;
    y: uint256;
}

contract StructCompound {
    p: Point;

    function mutate() returns (uint256) {
        p = Point(100, 200);
        return 5;
    }

    // The original struct is loaded before evaluating the right-hand side.
    // #[() -> 1520]
    function compound() public returns (uint256) {
        p = Point(10, 20);
        p.x += mutate();
        return p.x * 100 + p.y;
    }

    // #[() -> 520]
    function plain() public returns (uint256) {
        p = Point(10, 20);
        p.x = mutate();
        return p.x * 100 + p.y;
    }
}
