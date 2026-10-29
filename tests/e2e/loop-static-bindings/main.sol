pragma solcore;

import * from std;
import * from std.dispatch;

function touch(value: word) returns (word) {
    assembly {
        mstore(0, value)
        value := add(value, 1)
    }
    return value;
}

contract LoopStaticBindings {
    seed: word;

    // #[() -> 7]
    function zeroIterations() public returns (uint256) {
        let seed: comptime<word> = 7;
        for (; false;) {}
        return uint256(seed);
    }

    // #[(0) -> 7]
    // #[(3) -> 28]
    function assemblyBody(count: uint256) public returns (uint256) {
        let seed: comptime<word> = 7;
        let sum: word = 0;
        for (let i = uint256(0); i < count; i = i + uint256(1)) {
            assembly {
                sum := add(sum, seed)
            }
        }
        return uint256(primAddWord(sum, seed));
    }

    // #[(0) -> 7]
    // #[(3) -> 31]
    function helperBody(count: uint256) public returns (uint256) {
        let seed: comptime<word> = 7;
        let sum: word = 0;
        for (let i = uint256(0); i < count; i = i + uint256(1)) {
            sum = touch(sum);
            sum = primAddWord(sum, seed);
        }
        return uint256(primAddWord(sum, seed));
    }

    // #[(0, 10) -> 20]
    // #[(3, 10) -> 23]
    function capturedValue(count: uint256, input: uint256) public returns (uint256) {
        let current: word = Typedef.rep(input);
        let get: function () returns (word) = lam () -> word {
            return current;
        };
        for (let i = uint256(0); i < count; i = i + uint256(1)) {
            assembly {
                current := add(current, 1)
            }
        }
        return uint256(primAddWord(get(), current));
    }

    // #[(0) -> 7]
    // #[(3) -> 7]
    function scopedShadows(count: uint256) public returns (uint256) {
        let seed: comptime<word> = 7;
        for (let i = uint256(0); i < count; i = i + uint256(1)) {
            {
                let seed: word = 100;
                assembly {
                    seed := add(seed, 1)
                }
            }
            match (Typedef.rep(i)) {
                case seed {
                    assembly {
                        seed := 99
                    }
                }
            }
        }
        return uint256(seed);
    }

    // #[(0) -> 7]
    // #[(3) -> 7]
    function loopInitializer(count: uint256) public returns (uint256) {
        let value: word = 7;
        for (let value: word = 10; count > uint256(0); count = count - uint256(1)) {
            value = primAddWord(value, 1);
        }
        return uint256(value);
    }
}
