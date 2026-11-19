pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

// A leading-dot constructor denotes the same constructor as its qualified
// spelling: when a comptime scrutinee is matched, in the tag written and
// tested at run time, and through a derived storage representation. `U.A` is
// spelled `U_A` internally, which is also the source name of `U.U_A`, so `.U_A`
// must not be read as `U.A`. A wrong arm returns 99.

enum E {
    A,
    B
}
enum U {
    A,
    U_A
}

function pickStatic(comptime e: E) returns (word) {
    match (e) {
        case .A {
            return 99;
        }
        case .B {
            return 31;
        }
    }
}

function runtimeWord() returns (word) {
    let size: word;
    assembly {
        size := calldatasize()
    }
    return size;
}

contract DotConstructorIdentity {
    stored: U;

    // #[() -> 30]
    function comptimeLet() public returns (uint256) {
        let e: comptime<E> = .B;
        match (e) {
            case E.A {
                return 99;
            }
            case E.B {
                return 30;
            }
        }
    }

    // #[() -> 31]
    function comptimeArgument() public returns (uint256) {
        return uint256(pickStatic(E.B));
    }

    // #[() -> 32]
    function staticDotValue() public returns (uint256) {
        let u: U = .U_A;
        match (u) {
            case U.A {
                return 99;
            }
            case U.U_A {
                return 32;
            }
        }
    }

    // #[() -> 33]
    function staticDotPattern() public returns (uint256) {
        let u: U = U.U_A;
        match (u) {
            case .A {
                return 99;
            }
            case .U_A {
                return 33;
            }
        }
    }

    // The scrutinee is unknown until run time (calldata is 4 bytes long).
    // #[() -> 34]
    function runtimeDotValue() public returns (uint256) {
        let u: U = U.A;
        if (runtimeWord() == 4) {
            u = .U_A;
        }
        match (u) {
            case U.A {
                return 99;
            }
            case U.U_A {
                return 34;
            }
        }
    }

    // #[() -> 35]
    function runtimeDotPattern() public returns (uint256) {
        let u: U = .A;
        if (runtimeWord() == 4) {
            u = U.U_A;
        }
        match (u) {
            case .U_A {
                return 35;
            }
            case .A {
                return 99;
            }
        }
    }

    // #[() -> 36]
    function storageRoundTrip() public returns (uint256) {
        let value: U = .U_A;
        stored = value;
        let loaded: U = stored;
        match (loaded) {
            case U.A {
                return 99;
            }
            case U.U_A {
                return 36;
            }
        }
    }
}
