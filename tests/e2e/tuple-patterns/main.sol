import * from std;
import * from std.dispatch;

enum TupleBox {
    TupleBox(uint256, (uint256, uint256))
}
struct TupleFields {
    first: uint256;
    pair: (uint256, uint256);
}

function firstAfterUnit(p: ((), (uint256, uint256))) returns (uint256) {
    match (p) {
        case ((), (x, y)) {
            return x;
        }
    }
}

function lastInTail(p: (uint256, uint256, uint256)) returns (uint256) {
    match (p) {
        case (x, rest) {
            match (rest) {
                case (y, z) {
                    return z;
                }
            }
        }
    }
}

contract TuplePatterns {
    // #[(7, 9) -> 7]
    // #[(0, 42) -> 0]
    function nested(x: uint256, y: uint256) public returns (uint256) {
        return firstAfterUnit(((), (x, y)));
    }

    // #[(1, 2, 3) -> 3]
    // #[(7, 8, 42) -> 42]
    function tail(x: uint256, y: uint256, z: uint256) public returns (uint256) {
        return lastInTail((x, (y, z)));
    }

    // #[(1, 2, 3) -> 3]
    function constructorTail(x: uint256, y: uint256, z: uint256) public returns (uint256) {
        let boxed = TupleBox(x, (y, z));
        match (boxed) {
            case TupleBox(a, (b, c)) {
                return c;
            }
        }
    }

    // #[(1, 2, 3) -> 2]
    function structTail(x: uint256, y: uint256, z: uint256) public returns (uint256) {
        let fields = TupleFields(x, (y, z));
        match (fields.pair) {
            case (a, b) {
                return a;
            }
        }
    }

    // Both boolean alternatives cover the same product despite the grouping.
    // #[(true, 7) -> 7]
    // #[(false, 7) -> 0]
    function covered(flag: bool, value: uint256) public returns (uint256) {
        match (((), (flag, value))) {
            case ((), (true, x)) {
                return x;
            }
            case ((), (false, x)) {
                return uint256(0);
            }
        }
    }
}
