import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

struct Point {
    x: uint256;
    y: uint256;
}

contract C {
    p: Point;

    function set(x: uint256, y: uint256) public {
        p = Point(x, y);
    }

    function update(x: uint256, y: uint256) public {
        p.x = x;
        p.y += y;
    }

    function sum() public returns (uint256) {
        return p.x + p.y;
    }
}
