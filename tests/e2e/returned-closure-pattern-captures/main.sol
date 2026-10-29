pragma solcore;

import * from std;
import * from std.dispatch;

enum LabelBox<a> {
    LabelBox(a)
}

function captureLabel() returns (function (word) returns (word)) {
    let label: comptime<word> = 17;
    return lam (value: word) -> word {
        match (value) {
            case comptime label {
                return 1;
            }
            default {
                return 0;
            }
        }
    };
}

function captureTupleLabel(x: word) returns (function (word) returns (word)) {
    let label: comptime<word> = 17;
    return lam (value: word) -> word {
        match ((value, 9)) {
            case (comptime label, local) {
                return primAddWord(x, local);
            }
            default {
                return x;
            }
        }
    };
}

function captureConstructorLabel(x: word) returns (function (word) returns (word)) {
    let label: comptime<word> = 17;
    return lam (value: word) -> word {
        match (LabelBox((value, 9))) {
            case (LabelBox((comptime label, local))) {
                return primAddWord(x, local);
            }
            default {
                return x;
            }
        }
    };
}

function captureShadowedLabel(x: word) returns (function (word) returns (word)) {
    let label: comptime<word> = 17;
    return lam (value: word) -> word {
        match ((9, value)) {
            case (label, comptime label) {
                return primAddWord(x, label);
            }
            default {
                return x;
            }
        }
    };
}

contract ReturnedClosurePatternCaptures {
    // #[(17) -> 1]
    // #[(23) -> 0]
    function labelOnly(input: uint256) public returns (uint256) {
        let label: word = 99;
        let callback = captureLabel();
        return uint256(callback(Typedef.rep(input)));
    }

    // #[(17) -> 26]
    // #[(23) -> 23]
    function tupleLabel(input: uint256) public returns (uint256) {
        let local: word = Typedef.rep(input);
        let label: word = 99;
        let callback = captureTupleLabel(local);
        local = 99;
        return uint256(callback(Typedef.rep(input)));
    }

    // #[(17) -> 26]
    // #[(23) -> 23]
    function constructorLabel(input: uint256) public returns (uint256) {
        let local: word = Typedef.rep(input);
        let label: word = 99;
        let callback = captureConstructorLabel(local);
        local = 99;
        return uint256(callback(Typedef.rep(input)));
    }

    // #[(17) -> 26]
    // #[(23) -> 23]
    function labelBeforeBinder(input: uint256) public returns (uint256) {
        let label: word = Typedef.rep(input);
        let callback = captureShadowedLabel(label);
        label = 99;
        return uint256(callback(Typedef.rep(input)));
    }
}
