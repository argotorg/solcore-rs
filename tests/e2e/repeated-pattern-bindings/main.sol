pragma solcore;

import * from std;
import * from std.dispatch;

enum Box<a> {
    Box(a)
}

contract RepeatedPatternBindings {
    // #[(false) -> false]
    // #[(true) -> true]
    function staticColumns(flag: bool) public returns (bool) {
        let known: word = 3;
        match (known, true) {
            case (value, value) {
                value = flag;
                return value;
            }
        }
    }

    // #[(false) -> false]
    // #[(true) -> true]
    function nestedStaticPattern(flag: bool) public returns (bool) {
        let known: word = 3;
        match (Box((known, true))) {
            case (Box((value, value))) {
                value = flag;
                return value;
            }
        }
    }

    // #[(false) -> false]
    // #[(true) -> true]
    function partialColumns(flag: bool) public returns (bool) {
        let known: word = 3;
        match (known, flag) {
            case (value, value) {
                value = flag;
                return value;
            }
        }
    }

    // #[(false, 17) -> false]
    // #[(true, 23) -> true]
    function runtimeColumns(flag: bool, input: uint256) public returns (bool) {
        match (Typedef.rep(input), flag) {
            case (value, value) {
                value = flag;
                return value;
            }
        }
    }

    // #[(false) -> 7]
    // #[(true) -> 7]
    function restoreOuter(flag: bool) public returns (uint256) {
        let value: word = 7;
        let known: word = 3;
        match (known, true) {
            case (value, value) {
                value = flag;
            }
        }
        return uint256(value);
    }
}
