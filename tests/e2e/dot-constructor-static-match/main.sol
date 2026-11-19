pragma solcore;

import * from std;
import * from std.dispatch;
import { Imported } from types;
import { Reexported } from facade;

// Every `match` below is decided at compile time. It must take the arm that
// run-time matching would take, however the constructor is spelled: qualified
// (`E.A`), with a leading dot (`.A`), or unqualified. A wrong arm returns 99,
// or 98 when there are two wrong arms.

enum E {
    A,
    B
}
enum F {
    A,
    B
}
enum P {
    N,
    S(word),
    T(word, word)
}
enum O {
    None,
    Some(E)
}
enum Maybe<a> {
    Nothing,
    Just(a)
}
enum W {
    inl(word),
    inr(word)
}
enum Box {
    Box(word),
    Empty
}

type Alias = E;

function isA(e: E) returns (bool) {
    match (e) {
        case .A {
            return true;
        }
        default {
            return false;
        }
    }
}

function isQualifiedA(e: E) returns (bool) {
    match (e) {
        case E.A {
            return true;
        }
        default {
            return false;
        }
    }
}

function makeB() returns (E) {
    return .B;
}

function pickStatic(comptime e: E) returns (word) {
    match (e) {
        case E.A {
            return 25;
        }
        default {
            return 99;
        }
    }
}

contract DotConstructorStaticMatch {
    // #[() -> 10]
    function dotValueQualifiedPattern() public returns (uint256) {
        let e: E = .A;
        match (e) {
            case E.A {
                return 10;
            }
            default {
                return 99;
            }
        }
    }

    // #[() -> 11]
    function qualifiedValueDotPattern() public returns (uint256) {
        let e: E = E.A;
        match (e) {
            case .A {
                return 11;
            }
            default {
                return 99;
            }
        }
    }

    // #[() -> 12]
    function dotValueDotPattern() public returns (uint256) {
        let e: E = .B;
        match (e) {
            case .A {
                return 99;
            }
            case .B {
                return 12;
            }
            default {
                return 98;
            }
        }
    }

    // #[() -> 13]
    function payloadDotValue() public returns (uint256) {
        let p: P = .S(13);
        match (p) {
            case P.S(y) {
                return uint256(y);
            }
            default {
                return 99;
            }
        }
    }

    // #[() -> 14]
    function payloadDotPattern() public returns (uint256) {
        let p: P = P.T(6, 8);
        match (p) {
            case .S(y) {
                return 99;
            }
            case .T(a, b) {
                return uint256(addWord(a, b));
            }
            default {
                return 98;
            }
        }
    }

    // #[() -> 15]
    function nestedInTuple() public returns (uint256) {
        let t: (E, word) = (.B, 15);
        match (t) {
            case (E.A, x) {
                return 99;
            }
            case (E.B, x) {
                return uint256(x);
            }
            default {
                return 98;
            }
        }
    }

    // #[() -> 16]
    function nestedInConstructorDotValue() public returns (uint256) {
        let o: O = .Some(.B);
        match (o) {
            case O.Some(E.A) {
                return 99;
            }
            case O.Some(E.B) {
                return 16;
            }
            default {
                return 98;
            }
        }
    }

    // #[() -> 17]
    function nestedInConstructorDotPattern() public returns (uint256) {
        let o: O = O.Some(E.B);
        match (o) {
            case .Some(.A) {
                return 99;
            }
            case .Some(.B) {
                return 17;
            }
            default {
                return 98;
            }
        }
    }

    // #[() -> 18]
    function genericEnum() public returns (uint256) {
        let m: Maybe<E> = .Just(.A);
        match (m) {
            case Maybe.Just(E.A) {
                return 18;
            }
            default {
                return 99;
            }
        }
    }

    // #[() -> 19]
    function multipleScrutinees() public returns (uint256) {
        let a: E = .B;
        let b: F = F.A;
        match (a, b) {
            case (E.A, .A) {
                return 99;
            }
            case (E.B, .A) {
                return 19;
            }
            default {
                return 98;
            }
        }
    }

    // `E` and `F` share constructor names; each column resolves `.A` and `.B`
    // by its own type.
    // #[() -> 20]
    function sharedConstructorNames() public returns (uint256) {
        let t: (E, F) = (.A, F.B);
        match (t) {
            case (.A, F.A) {
                return 99;
            }
            case (E.A, .B) {
                return 20;
            }
            default {
                return 98;
            }
        }
    }

    // #[() -> 21]
    function importedEnum() public returns (uint256) {
        let i: Imported = Imported.B;
        match (i) {
            case .A {
                return 99;
            }
            case .B {
                return 21;
            }
            default {
                return 98;
            }
        }
    }

    // #[() -> 22]
    function reexportedEnum() public returns (uint256) {
        let r: Reexported = .B(22);
        match (r) {
            case Reexported.B(x) {
                return uint256(x);
            }
            default {
                return 99;
            }
        }
    }

    // #[() -> 23]
    function aliasedEnum() public returns (uint256) {
        let e: Alias = E.A;
        match (e) {
            case .A {
                return 23;
            }
            default {
                return 99;
            }
        }
    }

    // #[() -> 24]
    function inlinedResult() public returns (uint256) {
        match (makeB()) {
            case E.A {
                return 99;
            }
            case E.B {
                return 24;
            }
            default {
                return 98;
            }
        }
    }

    // #[() -> 25]
    function comptimeArgument() public returns (uint256) {
        return uint256(pickStatic(.A));
    }

    // #[() -> 26]
    function ifCondition() public returns (uint256) {
        if (isA(E.A)) {
            return 26;
        } else {
            return 99;
        }
    }

    // #[() -> 27]
    function conditionalExpression() public returns (uint256) {
        return isQualifiedA(.A) ? uint256(27) : uint256(99);
    }

    // #[() -> 28]
    function insideLambda() public returns (uint256) {
        let f = lam (e: E) -> word {
            match (e) {
                case .A {
                    return 28;
                }
                default {
                    return 99;
                }
            }
        };
        return uint256(f(E.A));
    }

    // An unqualified constructor that shares a builtin's name resolves by the
    // scrutinee type too.
    // #[() -> 29]
    function builtinNamedConstructor() public returns (uint256) {
        let w: W = W.inl(29);
        match (w) {
            case inl(x) {
                return uint256(x);
            }
            default {
                return 99;
            }
        }
    }

    // A constructor named like its type may be written unqualified.
    // #[() -> 30]
    function sameNameConstructorValue() public returns (uint256) {
        let b: Box = Box(30);
        match (b) {
            case .Box(v) {
                return uint256(v);
            }
            default {
                return 99;
            }
        }
    }

    // #[() -> 31]
    function sameNameConstructorPattern() public returns (uint256) {
        let b: Box = .Box(31);
        match (b) {
            case Box(v) {
                return uint256(v);
            }
            default {
                return 99;
            }
        }
    }
}
