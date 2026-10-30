pragma solcore;

import * from std;
import * from std.dispatch;

contract IfAsm {
    function f(b: bool) returns (word) {
        let x: word = 1;
        if (b) {
            assembly {
                x := 5
            }
        }
        return x;
    }

    function main() public returns (word) {
        let raw: word = 0;
        assembly {
            raw := calldataload(0)
        }
        let b: bool = tobool(raw);
        return f(b);
    }

}
