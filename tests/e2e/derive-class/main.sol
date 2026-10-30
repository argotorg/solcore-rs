pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;

pragma no-patterson-condition;
pragma no-bounded-variable-condition;

trait CloneLike<a> {
    function clone(x: a) returns (a);
}

impl CloneLike<uint256> {
    function clone(x: uint256) returns (uint256) {
        return x;
    }
}

contract DeriveClass {
    #[derive(Eq, Ord)]
    enum Color {
        Red,
        Green,
        Blue
    }

    #[derive(Eq, Ord)]
    enum Point {
        Point(uint256, uint256)
    }

    #[derive(CloneLike)]
    enum Box {
        Box(uint256)
    }

    constructor() {}

    // #[() -> 1]
    function eqRedRed() public returns (uint256) {
        match (Eq.eq(Color.Red, Color.Red)) {
            case true {
                return uint256(1);
            }
            case false {
                return uint256(0);
            }
        }
    }

    // #[() -> 0]
    function eqRedBlue() public returns (uint256) {
        match (Eq.eq(Color.Red, Color.Blue)) {
            case true {
                return uint256(1);
            }
            case false {
                return uint256(0);
            }
        }
    }

    // #[() -> 1]
    function gtGreenRed() public returns (uint256) {
        match (Ord.gt(Color.Green, Color.Red)) {
            case true {
                return uint256(1);
            }
            case false {
                return uint256(0);
            }
        }
    }

    // #[() -> 0]
    function gtRedGreen() public returns (uint256) {
        match (Ord.gt(Color.Red, Color.Green)) {
            case true {
                return uint256(1);
            }
            case false {
                return uint256(0);
            }
        }
    }

    // #[() -> 1]
    function eqPointSame() public returns (uint256) {
        match (Eq.eq(Point(uint256(1), uint256(2)), Point(uint256(1), uint256(2)))) {
            case true {
                return uint256(1);
            }
            case false {
                return uint256(0);
            }
        }
    }

    // #[() -> 1]
    function gtPointLex() public returns (uint256) {
        match (Ord.gt(Point(uint256(1), uint256(100)), Point(uint256(1), uint256(50)))) {
            case true {
                return uint256(1);
            }
            case false {
                return uint256(0);
            }
        }
    }

    // #[(42) -> 42]
    // #[(3735928559) -> 3735928559]
    function clonePayload(x: uint256) public returns (uint256) {
        match (CloneLike.clone(Box(x))) {
            case Box(value) {
                return value;
            }
        }
    }
}
