pragma solcore;

import * from std;
import * from std.dispatch;

contract C {
    function without_else(flag: bool, value: uint256) public returns (uint256) {
        let k: word = Typedef.rep(value);
        if (flag) {
            assembly {
                k := add(k, 1)
            }
        }
        return uint256(k);
    }

    function with_else(flag: bool, value: uint256) public returns (uint256) {
        let k: word = Typedef.rep(value);
        if (flag) {
            assembly {
                k := add(k, 1)
            }
        } else {
            assembly {
                k := add(k, 2)
            }
        }
        return uint256(k);
    }
}
