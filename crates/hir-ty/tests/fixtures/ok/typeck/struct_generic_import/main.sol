pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from points;

type WordPoint = Point<word>;

contract StructFields {
    count: uint256;

    #[derive(Eq)]
    struct Local {
        value: uint256;
    }

    constructor() {
        count = 0;
    }

    function makePoint() returns (WordPoint) {
        count = count + 1;
        return Point(17, 25);
    }

    // A projection evaluates a side-effecting receiver exactly once.
    // #[() -> 18]
    function receiverOnce() public returns (uint256) {
        let x: word = makePoint().x;
        return uint256(x) + count;
    }

    // Imported generic fields, aliases, nested reads and match all share a product representation.
    // #[() -> 42]
    function nested() public returns (uint256) {
        let line: Line<word> = Line(Point(1, 2), Point(20, 22));
        match (line.end) {
            case Point(a, b) {
                return uint256(a) + uint256(b);
            }
        }
    }

    // #[() -> 20]
    function chained() public returns (uint256) {
        let line: Line<word> = Line(Point(1, 2), Point(20, 22));
        return uint256(line.end.x);
    }

    // #[() -> true]
    function derived() public returns (bool) {
        return Eq.eq(Local(42), Local(42));
    }
}
