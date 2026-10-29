pragma solcore;

import * from std;
import * from std.dispatch;

enum Box {
    Box(uint256, uint256)
}

contract PatternBindingOrder {
    // #[(1, 2) -> 2]
    // #[(7, 9) -> 9]
    function swap(x: uint256, y: uint256) public returns (uint256) {
        match (x, y) {
            case (y, x) {
                return x;
            }
        }
    }

    // #[(1, 2, 3) -> 3]
    function cycle(x: uint256, y: uint256, z: uint256) public returns (uint256) {
        match (x, y, z) {
            case (y, z, x) {
                return x;
            }
        }
    }

    // #[(1, 2, 3) -> 3]
    function nested(x: uint256, y: uint256, z: uint256) public returns (uint256) {
        match ((x, (y, z))) {
            case (z, (x, y)) {
                return y;
            }
        }
    }

    // #[(1, 2) -> 2]
    function boxed(x: uint256, y: uint256) public returns (uint256) {
        let box = Box(x, y);
        match (box, y) {
            case (Box(y, x), box) {
                return x;
            }
        }
    }

    // #[(1, 2) -> 12]
    // #[(7, 9) -> 79]
    function outerBindings(x: uint256, y: uint256) public returns (uint256) {
        let observed = uint256(0);
        match (x, y) {
            case (y, x) {
                observed = x;
            }
        }
        return x * uint256(10) + observed;
    }

    // #[(1, 2, 3) -> 123]
    function nestedScopes(x: uint256, y: uint256, z: uint256) public returns (uint256) {
        let observed = uint256(0);
        match (x, y) {
            case (y, x) {
                match ((x, z)) {
                    case (z, x) {
                        observed = x;
                    }
                }
                observed = x * uint256(10) + observed;
            }
        }
        return x * uint256(100) + observed;
    }
}
