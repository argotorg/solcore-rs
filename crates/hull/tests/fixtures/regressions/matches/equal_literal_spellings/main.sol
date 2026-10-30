pragma solcore;

contract C {
    function pick(x: word) returns (word) {
        match (x) {
            case 0x2a {
                return 111;
            }
            case 0042 {
                return 222;
            }
            default {
                return 333;
            }
        }
    }

    function main() returns (word) {
        let x: word = 0;
        assembly {
            x := calldataload(0)
        }
        return pick(x);
    }
}
