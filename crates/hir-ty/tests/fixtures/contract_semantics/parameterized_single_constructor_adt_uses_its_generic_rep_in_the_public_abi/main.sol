pragma solcore;

import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Point<a> {
    Point(a, bool)
}

contract Shapes {
    function roundtrip(p: Point<uint256>) public returns (Point<uint256>) {
        return p;
    }
}
