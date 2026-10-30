pragma solcore;

enum Tiny {
    A,
    B,
    C
}

contract C {
    function pick(t: Tiny) public returns (word) {
        assembly {
            mstore(0, 0)
        }
        match (t) {
            case Tiny.B {
                return 2;
            }
            default {
                return 9;
            }
        }
    }

    function main() returns (word) {
        return pick(Tiny.B);
    }
}
