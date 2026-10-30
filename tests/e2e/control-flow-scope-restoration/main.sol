pragma solcore;

import * from std;
import * from std.dispatch;

function requireStatic(comptime x: word) returns (word) {
    return x;
}

contract ControlFlowScopeRestoration {
    // #[(false) -> 7]
    // #[(true) -> 7]
    function conditional(flag: bool) public returns (uint256) {
        let seed: comptime<word> = 7;
        {
            if (flag) {
                let seed: word = 1;
            } else {}
        }
        return uint256(requireStatic(seed));
    }

    // #[(false) -> 7]
    // #[(true) -> 7]
    function loopDeclaration(flag: bool) public returns (uint256) {
        let seed: comptime<word> = 7;
        {
            for (let seed: word = 1; flag; seed = 2) {
                seed = 3;
                break;
            }
        }
        return uint256(requireStatic(seed));
    }

    // #[(false) -> 7]
    // #[(true) -> 7]
    function knownMatch(flag: bool) public returns (uint256) {
        let seed: comptime<word> = 7;
        match (true) {
            case true {
                if (flag) {
                    let seed: word = 1;
                } else {}
            }
            case false {}
        }
        return uint256(requireStatic(seed));
    }

    // #[(false) -> 7]
    // #[(true) -> 7]
    function unknownMatch(flag: bool) public returns (uint256) {
        let seed: comptime<word> = 7;
        match (flag) {
            case true {
                {
                    if (flag) {
                        let seed: word = 1;
                    } else {}
                }
                let checked: comptime<word> = requireStatic(seed);
            }
            case false {}
        }
        return uint256(requireStatic(seed));
    }

    // A write before the control flow must survive the enclosing block.
    // #[(false) -> 9]
    // #[(true) -> 9]
    function priorWrite(flag: bool) public returns (uint256) {
        let seed: word = 7;
        {
            seed = 9;
            if (flag) {
                let seed: word = 1;
            } else {}
        }
        return uint256(requireStatic(seed));
    }

    // A conditional write before shadowing invalidates the saved outer value.
    // #[(false) -> 7]
    // #[(true) -> 9]
    function conditionalOuterWrite(flag: bool) public returns (uint256) {
        let seed: word = 7;
        {
            if (flag) {
                seed = 9;
                let seed: word = 1;
            } else {}
        }
        return uint256(seed);
    }

    // Each branch owns its declarations; the else assignment writes the outer seed.
    // #[(false) -> 3]
    // #[(true) -> 7]
    function writesAfterShadow(flag: bool) public returns (uint256) {
        let seed: word = 7;
        {
            if (flag) {
                let seed: word = 1;
                seed = 2;
            } else {
                seed = 3;
            }
        }
        return uint256(seed);
    }

    // The post clause is resolved before the body declaration, and writes outer seed.
    // #[(false) -> 7]
    // #[(true) -> 9]
    function loopOuterWrite(flag: bool) public returns (uint256) {
        let seed: word = 7;
        let running: bool = flag;
        {
            for (; running; seed = 9) {
                running = false;
                let seed: word = 1;
            }
        }
        return uint256(seed);
    }

    // Restore callable bindings as well as constants, retaining their captured value.
    // #[(false, 13) -> 13]
    // #[(true, 29) -> 29]
    function callable(flag: bool, n: uint256) public returns (uint256) {
        let get: function () returns (uint256) = lam () -> uint256 {
            return n;
        };
        {
            if (flag) {
                let get: word = 1;
            } else {}
        }
        return get();
    }
    // The initializer still refers to the binding before the declaration.
    // #[(false) -> 0]
    // #[(true) -> 7]
    function initializedShadow(flag: bool) public returns (uint256) {
        let seed: comptime<word> = 7;
        let result: word = 0;
        {
            if (flag) {
                let seed: word = seed;
                result = seed;
            } else {}
        }
        return uint256(result);
    }

    // #[(false) -> 0]
    // #[(true) -> 7]
    function readBeforeShadow(flag: bool) public returns (uint256) {
        let seed: comptime<word> = 7;
        let result: word = 0;
        {
            if (flag) {
                result = seed;
                let seed: word = 1;
            } else {}
        }
        return uint256(result);
    }

    // #[(false) -> 0]
    // #[(true) -> 7]
    function loopInitializedShadow(flag: bool) public returns (uint256) {
        let seed: comptime<word> = 7;
        let result: word = 0;
        {
            for (let seed: word = seed; flag;) {
                result = seed;
                break;
            }
        }
        return uint256(result);
    }

    // #[(false) -> 0]
    // #[(true) -> 7]
    function loopBodyInitializedShadow(flag: bool) public returns (uint256) {
        let seed: comptime<word> = 7;
        let result: word = 0;
        {
            for (; flag;) {
                let seed: word = seed;
                result = seed;
                break;
            }
        }
        return uint256(result);
    }

    // Saving an evaluated arm's value would incorrectly restore the dead write.
    // #[() -> 7]
    function knownDeadWrite() public returns (uint256) {
        let seed: word = 7;
        {
            if (false) {
                seed = 9;
                let seed: word = 1;
            } else {}
        }
        return uint256(seed);
    }

    // The selected arm's write is known when no discarded arm introduces bindings.
    // #[() -> 9]
    function knownLiveWrite() public returns (uint256) {
        let seed: word = 7;
        {
            if (true) {
                seed = 9;
                let seed: word = 1;
            } else {}
        }
        return uint256(requireStatic(seed));
    }

}
