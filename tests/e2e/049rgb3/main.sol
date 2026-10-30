pragma solcore;

import * from std;
import * from std.dispatch;

enum RGB {
    Red(word),
    Green(word),
    Blue(word)
}

contract RGB3 {

    function choose(c: RGB) returns (word) {
        let res: word;
        match (c) {
            case .Red(x) {
                assembly {
                    res := add(x, 1)
                }
            }
            case .Green(x) {
                assembly {
                    res := add(x, 2)
                }
            }
            case .Blue(x) {
                assembly {
                    res := add(x, 3)
                }
            }
        }
        return res;
    }
    // #[() -> 44]
    function run() public returns (uint256) {
        return uint256(choose(RGB.Green(42)));
    }
}
