pragma solcore;

import * from std;
import * from std.dispatch;

contract RetUnknown {
    function pick(flag: bool, y: word) returns (word) {
        match (flag) {
            case true {
                return y;
            }
            case false {
                return 5;
            }
        }
        return 0;
    }

    function get(x: word) returns (word) {
        return pick(true, x);
    }

    function main() public returns (word) {
        let x: word = 0;
        assembly {
            x := calldataload(0)
        }
        return get(x);
    }

}
