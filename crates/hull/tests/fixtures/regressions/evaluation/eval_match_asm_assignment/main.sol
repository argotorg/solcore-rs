pragma solcore;

import * from std;
import * from std.dispatch;

contract MatchAsm {
    function g(b: bool) returns (word) {
        let x: word = 1;
        match (b) {
            case true {
                assembly {
                    x := 5
                }
            }
            case false {}
        }
        return x;
    }

    function main() public returns (word) {
        let raw: word = 0;
        assembly {
            raw := calldataload(0)
        }
        let b: bool = tobool(raw);
        return g(b);
    }

}
