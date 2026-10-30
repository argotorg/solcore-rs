pragma solcore;

//~ specialize: loader fixture

import * from std;
import * from std.dispatch;

enum Box<a> {
    Box(a)
}
enum Choice {
    Good(word),
    Bad(word)
}

function takeStatic(comptime value: word) returns (word) {
    return value;
}

function unwrapStatic(comptime value: Box<Box<word>>) returns (word) {
    match (value) {
        case (Box(Box(inner))) {
            return takeStatic(inner);
        }
    }
}

function unwrapTuple(comptime value: (word, word)) returns (word) {
    match (value) {
        case ((inner, _)) {
            return takeStatic(inner);
        }
    }
}

contract ComptimePatternScopes {
    // #[(false) -> 7]
    // #[(true) -> 7]
    function shadowedStatic(flag: bool) public returns (uint256) {
        let seed: comptime<word> = 7;
        match (flag) {
            case (seed) {
                seed = true;
            }
        }
        let value: word = ((flag) ? 1 : 2);
        match (Box(value)) {
            case (Box(seed)) {
                assembly {
                    seed := 0
                }
            }
        }
        return uint256(seed);
    }

    // #[() -> 3]
    function destructuredStatic() public returns (uint256) {
        return uint256(unwrapStatic(Box(Box(3))));
    }

    // #[() -> 3]
    function destructuredTuple() public returns (uint256) {
        return uint256(unwrapTuple((3, 9)));
    }

    // #[(false) -> 3]
    // #[(true) -> 3]
    function mixedScrutinees(flag: bool) public returns (uint256) {
        let seed: comptime<word> = 3;
        match (Box(seed), flag) {
            case (Box(inner), seed) {
                seed = false;
                return uint256(takeStatic(inner));
            }
        }
    }

    // #[(false) -> 7]
    // #[(true) -> 9]
    function outerAssignment(flag: bool) public returns (uint256) {
        let value: word = 7;
        match (flag) {
            case (true) {
                value = 9;
            }
            case (false) {}
        }
        return uint256(value);
    }

    // #[(false) -> 5]
    // #[(true) -> 4]
    function mutableStaticPattern(flag: bool) public returns (uint256) {
        match (Choice.Good(3), flag) {
            case (Choice.Bad(_), _) {
                return uint256(99);
            }
            case (Choice.Good(inner), selected) {
                inner = primAddWord(inner, ((selected) ? 1 : 2));
                return uint256(inner);
            }
        }
    }

    // #[(false, 17) -> 17]
    // #[(true, 23) -> 23]
    function capturedStaticPattern(flag: bool, input: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let get: function () returns (word) = lam () -> word {
            return value;
        };
        match (Box(get), flag) {
            case (Box(invoke), value) {
                return uint256(invoke());
            }
        }
    }

    // #[(false, 17) -> 17]
    // #[(true, 23) -> 23]
    function directCapturedPattern(flag: bool, input: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        match (Box(lam () -> word {
                    return value;
                }), flag) {
            case (Box(invoke), value) {
                return uint256(invoke());
            }
        }
    }

    // #[(false, 17) -> 17]
    // #[(true, 23) -> 23]
    function nestedCapturedPattern(flag: bool, input: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        match (Box(lam () -> word {
                    return value;
                }), flag) {
            case (box, value) {
                match (box) {
                    case (Box(invoke)) {
                        return uint256(invoke());
                    }
                }
            }
        }
    }

    // #[(17) -> 17]
    function repeatedRuntimeBinder(input: uint256) public returns (uint256) {
        let known: word = 3;
        match (known, Typedef.rep(input)) {
            case (value, value) {
                return uint256(value);
            }
        }
    }

    // #[(17) -> 3]
    function repeatedStaticBinder(input: uint256) public returns (uint256) {
        let known: word = 3;
        match (Typedef.rep(input), known) {
            case (value, value) {
                return uint256(value);
            }
        }
    }
}
