pragma solcore;

import * from std;
import * from std.dispatch;

function requireStatic(comptime x: word) returns (word) {
    return x;
}

function skippedInitializer() returns (word) {
    assembly {
        sstore(0, 99)
    }
    return 11;
}

function captureCaller(x: word) returns (function () returns (word)) {
    let y: word;
    return lam () -> word {
        if (true) {
            return x;
        } else {
            let result: word;
            assembly {
                result := y
            }
            return result;
        }
    };
}

function readSkipped() returns (word) {
    let x: word = 7;
    if (false) {
        let x: word = 9;
        return x;
    } else {
        return x;
    }
}

contract KnownIfBindingOrder {
    // #[() -> 7]
    function selectedConstant() public returns (uint256) {
        let x: word = 1;
        if (true) {
            x = 7;
        } else {
            let unused: word = skippedInitializer();
            let callback: function () returns (word) = lam () -> word {
                return 9;
            };
            let staticValue: comptime<word> = 9;
        }
        let changed: word;
        assembly {
            changed := sload(0)
        }
        return uint256(requireStatic(x) + changed);
    }

    // #[() -> 9]
    function restoreWrittenOuter() public returns (uint256) {
        let x: word = 7;
        {
            if (true) {
                x = 9;
            } else {
                let x: word = 1;
            }
        }
        return uint256(requireStatic(x));
    }

    // #[() -> 7]
    function unavailableRead() public returns (uint256) {
        return uint256(readSkipped());
    }

    // #[() -> 7]
    function unavailableAssignment() public returns (uint256) {
        let x: word = 0;
        if (false) {
            let x: word = 9;
        } else {
            assembly {
                x := add(x, 7)
            }
        }
        return uint256(x);
    }

    // #[(13) -> 13]
    // #[(37) -> 37]
    function symbolicCaller(value: uint256) public returns (uint256) {
        let y: word = Typedef.rep(value);
        let callback = captureCaller(y);
        return uint256(callback());
    }
}
