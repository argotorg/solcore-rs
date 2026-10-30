pragma solcore;

import * from std;
import * from std.dispatch;

enum CallbackBox<a> {
    CallbackBox(a)
}

function forward<a>(value: a) returns (a) {
    return value;
}

function captureParameter(x: word) returns (function (word) returns (word)) {
    return lam (y: word) -> word {
        return primAddWord(x, y);
    };
}

function captureLocal(x: word) returns (function () returns (word)) {
    return lam () -> word {
        let y: word = primAddWord(x, 1);
        return primAddWord(x, y);
    };
}

function capturePattern(x: word) returns (function (word) returns (word)) {
    return lam (value: word) -> word {
        match ((value, 1)) {
            case (y, extra) {
                return primAddWord(x, primAddWord(y, extra));
            }
        }
    };
}

function captureBlock(x: word) returns (function () returns (word)) {
    return lam () -> word {
        let total: word = x;
        {
            let y: word = 9;
            total = primAddWord(total, primAddWord(x, y));
        }
        return primAddWord(total, x);
    };
}

function captureIf(x: word) returns (function (bool) returns (word)) {
    return lam (flag: bool) -> word {
        let y: word = 1;
        if (flag) {
            let y: word = 9;
        } else {
            y = 3;
        }
        return primAddWord(x, y);
    };
}

function captureLoop(x: word) returns (function () returns (word)) {
    return lam () -> word {
        let y: word = 0;
        for (let i: uint256 = uint256(0); i < uint256(1); i = i + uint256(1)) {
            y = 9;
        }
        return primAddWord(x, y);
    };
}

function captureNested(x: word) returns (CallbackBox<function (word) returns (word)>) {
    return CallbackBox(lam (y: word) -> word {
            let inner = lam (z: word) -> word {
                return primAddWord(x, primAddWord(y, z));
            };
            return inner(2);
        });
}

function captureValue(x: word) returns (function () returns (word)) {
    return lam () -> word {
        return x;
    };
}

function captureWrite(x: word) returns (function (word) returns (word)) {
    return lam (y: word) -> word {
        x = primAddWord(x, 1);
        return primAddWord(x, y);
    };
}

function wrap(callback: function (word) returns (word)) returns (function (word) returns (word)) {
    return lam (y: word) -> word {
        return callback(y);
    };
}

contract ReturnedClosureSubstitution {
    // #[(17) -> 26]
    // #[(23) -> 32]
    function parameter(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let callback = captureParameter(y);
        return uint256(callback(9));
    }

    // #[(17) -> 35]
    // #[(23) -> 47]
    function localInitializer(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let callback = captureLocal(y);
        return uint256(callback());
    }

    // #[(17) -> 27]
    // #[(23) -> 33]
    function pattern(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let callback = capturePattern(y);
        return uint256(callback(9));
    }

    // #[(17) -> 60]
    // #[(23) -> 78]
    function block(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let callback = captureBlock(y);
        return uint256(callback());
    }

    // #[(17, true) -> 18]
    // #[(17, false) -> 20]
    // #[(23, true) -> 24]
    // #[(23, false) -> 26]
    function sharedIf(input: uint256, flag: bool) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let callback = captureIf(y);
        return uint256(callback(flag));
    }

    // #[(17) -> 26]
    // #[(23) -> 32]
    function sharedLoop(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let callback = captureLoop(y);
        return uint256(callback());
    }

    // #[(17) -> 28]
    // #[(23) -> 34]
    function nestedAggregate(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let boxed = forward(captureNested(y));
        y = 99;
        match (boxed) {
            case (CallbackBox(callback)) {
                return uint256(callback(9));
            }
        }
    }

    // #[(17) -> 1718]
    // #[(23) -> 2324]
    function independentCaptures(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let first = captureValue(y);
        y = primAddWord(y, 1);
        let second = captureValue(y);
        y = 99;
        return uint256(first()) * uint256(100) + uint256(second());
    }

    // #[(17) -> 44]
    // #[(23) -> 56]
    function capturedWrite(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let callback = captureWrite(y);
        return uint256(callback(9)) + uint256(y);
    }

    // #[(17) -> 26]
    // #[(23) -> 32]
    function wrappedCallable(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let callback = captureParameter(y);
        let boxed = forward(CallbackBox(wrap(forward(callback))));
        y = 99;
        match (boxed) {
            case (CallbackBox(invoke)) {
                return uint256(invoke(9));
            }
        }
    }

    // #[(17) -> 27]
    // #[(23) -> 33]
    function computedCapture(input: uint256) public returns (uint256) {
        let y: word = Typedef.rep(input);
        let callback = captureParameter(primAddWord(y, 1));
        y = 99;
        return uint256(callback(9));
    }
}
