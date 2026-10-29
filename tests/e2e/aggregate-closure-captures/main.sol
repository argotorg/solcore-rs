pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch

import * from std;
import * from std.dispatch;

enum Box<a> {
    Box(a)
}
struct CallableRecord {
    invoke: function () returns (word);
    extra: word;
}

function forward<a>(value: a) returns (a) {
    return value;
}
function takeStatic(comptime value: word) returns (word) {
    return value;
}

function makeBox(input: word) returns (Box<function () returns (word)>) {
    let value: word = input;
    let boxed = Box(lam () -> word {
            return value;
        });
    value = 9;
    return boxed;
}

function repeatBox(boxed: Box<function () returns (word)>, count: uint256) returns (word) {
    if (count == uint256(0)) {
        match (boxed) {
            case (Box(invoke)) {
                return invoke();
            }
        }
    }
    return repeatBox(boxed, count - uint256(1));
}

function repeatPayload(boxed: Box<(function () returns (word), word)>, count: uint256) returns (word) {
    match (boxed) {
        case (Box((invoke, index))) {
            if (count == uint256(0)) {
                return primAddWord(invoke(), index);
            }
            return repeatPayload(Box((invoke, primAddWord(index, 1))), count - uint256(1));
        }
    }
}

contract AggregateClosureCaptures {
    count: uint256;

    function next() returns (word) {
        count = count + uint256(1);
        return Typedef.rep(count);
    }

    function invokeBox(boxed: Box<(function () returns (word), word)>) returns (word) {
        count = count + uint256(1);
        match (boxed) {
            case (Box((invoke, extra))) {
                return primAddWord(invoke(), extra);
            }
        }
    }
    // #[(17, false) -> 17]
    // #[(23, true) -> 23]
    function delayed(input: uint256, flag: bool) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let boxed = Box(lam () -> word {
                return value;
            });
        value = 9;
        match (boxed, flag) {
            case (Box(invoke), _) {
                return uint256(invoke());
            }
        }
    }

    function invokeStatic(comptime boxed: Box<(function () returns (word), word)>) returns (word) {
        count = count + uint256(1);
        match (boxed) {
            case (Box((invoke, extra))) {
                return primAddWord(invoke(), takeStatic(extra));
            }
        }
    }

    // #[(17, false) -> 17]
    // #[(23, true) -> 23]
    function comptimeAggregate(input: uint256, flag: bool) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let boxed: comptime<Box<function () returns (word)>> =
        Box(lam () -> word {
                return value;
            });
        value = 9;
        match (boxed, flag) {
            case (Box(invoke), _) {
                return uint256(invoke());
            }
        }
    }

    // #[(17) -> 17]
    // #[(23) -> 23]
    function comptimeCallable(input: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let invoke: comptime<function () returns (word)> = lam () -> word {
            return value;
        };
        value = 9;
        return uint256(invoke());
    }

    // #[(17) -> 17]
    // #[(23) -> 23]
    function nestedTuple(input: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let boxed = Box((true, Box(lam () -> word {
                        return value;
                    })));
        value = 9;
        match (boxed) {
            case (Box((_, Box(invoke)))) {
                return uint256(invoke());
            }
        }
    }

    // #[(17) -> 17]
    // #[(23) -> 23]
    function assignedAggregate(input: uint256) public returns (uint256) {
        let value: word = 1;
        let boxed = Box(lam () -> word {
                return value;
            });
        value = Typedef.rep(input);
        boxed = Box(lam () -> word {
                return value;
            });
        value = 9;
        match (boxed) {
            case (Box(invoke)) {
                return uint256(invoke());
            }
        }
    }

    // #[(17) -> 17]
    // #[(23) -> 23]
    function assignedCallable(input: uint256) public returns (uint256) {
        let value: word = 1;
        let invoke: function () returns (word) = lam () -> word {
            return value;
        };
        value = Typedef.rep(input);
        invoke = lam () -> word {
            return value;
        };
        value = 9;
        return uint256(invoke());
    }

    // #[(17) -> 17]
    // #[(23) -> 23]
    function forwardedAggregate(input: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let boxed = forward(Box(lam () -> word {
                    return value;
                }));
        value = 9;
        match (boxed) {
            case (Box(invoke)) {
                return uint256(invoke());
            }
        }
    }

    // #[(17) -> 17]
    // #[(23) -> 23]
    function capturedBeforeShadow(input: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let boxed = Box(lam () -> word {
                return value;
            });
        let value: bool = true;
        match (boxed) {
            case (Box(invoke)) {
                return uint256(invoke());
            }
        }
    }

    // #[(17, 3) -> 20]
    // #[(23, 5) -> 28]
    function runtimePayload(input: uint256, extra: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let payload: word = Typedef.rep(extra);
        let boxed = Box((lam () -> word {
                    return value;
                }, payload));
        value = 9;
        payload = 11;
        match (boxed) {
            case (Box((invoke, saved))) {
                return uint256(primAddWord(invoke(), saved));
            }
        }
    }

    // #[(17, 0) -> 17]
    // #[(17, 3) -> 3]
    function runtimePattern(input: uint256, extra: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let boxed = Box((lam () -> word {
                    return value;
                }, Typedef.rep(extra)));
        value = 9;
        match (boxed) {
            case (Box((invoke, 0))) {
                return uint256(invoke());
            }
            case (Box((_, saved))) {
                return uint256(saved);
            }
        }
    }

    // #[(17, 3, false) -> 20]
    // #[(23, 5, true) -> 28]
    function nestedBinding(input: uint256, extra: uint256, flag: bool) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let boxed = Box((lam () -> word {
                    return value;
                }, Typedef.rep(extra)));
        value = 9;
        match (boxed, flag) {
            case (bound, _) {
                match (bound) {
                    case (Box((invoke, saved))) {
                        return uint256(primAddWord(invoke(), saved));
                    }
                }
            }
        }
    }

    // #[(17) -> 17]
    // #[(23) -> 23]
    function factory(input: uint256) public returns (uint256) {
        let boxed = makeBox(Typedef.rep(input));
        input = uint256(9);
        match (boxed) {
            case (Box(invoke)) {
                return uint256(invoke());
            }
        }
    }

    // #[(17, 0) -> 17]
    // #[(23, 4) -> 23]
    function recursiveArgument(input: uint256, times: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let boxed = Box(lam () -> word {
                return value;
            });
        value = 9;
        return uint256(repeatBox(boxed, times));
    }

    // #[(17, 0) -> 17]
    // #[(23, 4) -> 27]
    function growingRecursivePayload(input: uint256, times: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let boxed: Box<(function () returns (word), word)> =
        Box((lam () -> word {
                    return value;
                }, 0));
        value = 9;
        return uint256(repeatPayload(boxed, times));
    }

    // #[(17) -> 20]
    // #[(23) -> 26]
    function effectfulPayload(input: uint256) public returns (uint256) {
        count = uint256(0);
        let value: word = Typedef.rep(input);
        let boxed: Box<(function () returns (word), word)> = Box((lam () -> word {
                    return value;
                }, next()));
        value = 9;
        let result: word = invokeBox(boxed);
        return uint256(primAddWord(result, Typedef.rep(count)));
    }

    // #[(17) -> 25]
    // #[(23) -> 31]
    function staticPayload(input: uint256) public returns (uint256) {
        count = uint256(0);
        let value: word = Typedef.rep(input);
        let boxed: Box<(function () returns (word), word)> =
        Box((lam () -> word {
                    return value;
                }, 7));
        value = 9;
        let result: word = invokeStatic(boxed);
        return uint256(primAddWord(result, Typedef.rep(count)));
    }

    // #[(17, 3) -> 20]
    // #[(23, 5) -> 28]
    function projectedFields(input: uint256, extra: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let record = CallableRecord(lam () -> word {
                return value;
            }, Typedef.rep(extra));
        value = 9;
        let invoke = record.invoke;
        return uint256(primAddWord(invoke(), record.extra));
    }

    // #[(17, 3) -> 20]
    // #[(23, 5) -> 28]
    function annotatedProjection(input: uint256, extra: uint256) public returns (uint256) {
        let value: word = Typedef.rep(input);
        let callable: function () returns (word) = lam () -> word {
            return value;
        };
        let record: CallableRecord = CallableRecord(callable, Typedef.rep(extra));
        value = 9;
        let invoke = record.invoke;
        return uint256(primAddWord(invoke(), record.extra));
    }

    // #[(17) -> 20]
    // #[(23) -> 26]
    function projectedEffectfulPayload(input: uint256) public returns (uint256) {
        count = uint256(0);
        let value: word = Typedef.rep(input);
        let record = CallableRecord(lam () -> word {
                return value;
            }, next());
        value = 9;
        let invoke = record.invoke;
        let total = primAddWord(invoke(), primAddWord(record.extra, record.extra));
        return uint256(primAddWord(total, Typedef.rep(count)));
    }
}
