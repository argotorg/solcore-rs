pragma solcore;

import * from std;
import * from std.dispatch;

contract C {
    function exact() returns (word) {
        return 115792089237316195423570985008687907853269984665640564039457584007913129639936;
    }

    function plus() returns (word) {
        return 115792089237316195423570985008687907853269984665640564039457584007913129639937;
    }

    function pick(x: word) returns (word) {
        match (x) {
            case 115792089237316195423570985008687907853269984665640564039457584007913129639936 {
                return 10;
            }
            case 115792089237316195423570985008687907853269984665640564039457584007913129639937 {
                return 11;
            }
            default {
                return 12;
            }
        }
    }

    function main() public returns (word) {
        let x: word = 0;
        assembly {
            x := calldataload(0)
        }
        return exact() + plus() + pick(x);
    }

}
