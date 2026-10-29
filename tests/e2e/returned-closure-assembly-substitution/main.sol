pragma solcore;

import * from std;
import * from std.dispatch;

function captureParameter(x: word) returns (function (word) returns (word)) {
    return lam (y: word) -> word {
        let out: word;
        assembly {
            out := add(x, y)
        }
        return out;
    };
}

function captureLocal(x: word) returns (function () returns (word)) {
    return lam () -> word {
        let out: word;
        assembly {
            let y := add(x, 1)
            out := add(x, y)
        }
        return out;
    };
}

function captureBlock(x: word) returns (function () returns (word)) {
    return lam () -> word {
        let out: word;
        assembly {
            out := x
            {
                let y := 9
                out := add(out, add(x, y))
            }
            out := add(out, x)
        }
        return out;
    };
}

function captureLoop(x: word) returns (function () returns (word)) {
    return lam () -> word {
        let out: word = 0;
        assembly {
            for {
                let y := 0
            } lt(y, 2) {
                y := add(y, 1)
            } {
                out := add(out, add(x, y))
            }
            out := add(out, x)
        }
        return out;
    };
}

function captureFunction(x: word) returns (function () returns (word)) {
    return lam () -> word {
        let out: word;
        assembly {
            function increment(x) -> y {
                y := add(x, 1)
            }
            out := add(increment(9), x)
        }
        return out;
    };
}

function captureWrite(x: word) returns (function (word) returns (word)) {
    return lam (y: word) -> word {
        let out: word;
        assembly {
            x := add(x, 1)
            out := add(x, y)
        }
        return out;
    };
}

contract ReturnedClosureAssemblySubstitution {
    // #[(17) -> 26]
    // #[(23) -> 32]
    function parameterAndSnapshot(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let x: word = 99;
        let callback = captureParameter(y);
        y = 99;
        return uint256(callback(9));
    }

    // #[(17) -> 35]
    // #[(23) -> 47]
    function localInitializer(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let x: word = 99;
        let callback = captureLocal(y);
        return uint256(callback());
    }

    // #[(17) -> 60]
    // #[(23) -> 78]
    function block(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let x: word = 99;
        let callback = captureBlock(y);
        return uint256(callback());
    }

    // #[(17) -> 52]
    // #[(23) -> 70]
    function loop(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let x: word = 99;
        let callback = captureLoop(y);
        return uint256(callback());
    }

    // #[(17) -> 27]
    // #[(23) -> 33]
    function functionIsolation(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let x: word = 99;
        let callback = captureFunction(y);
        return uint256(callback());
    }

    // #[(17) -> 143]
    // #[(23) -> 155]
    function capturedWrite(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let x: word = 99;
        let callback = captureWrite(y);
        return uint256(callback(9)) + uint256(y) + uint256(x);
    }
}
