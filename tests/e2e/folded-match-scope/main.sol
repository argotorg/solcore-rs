pragma solcore;

import * from std;
import * from std.dispatch;

enum Pair {
    Pair(word, word)
}

contract FoldedMatchScope {
    // #[() -> 3]
    function simultaneousBindings() public returns (uint256) {
        let x: word = 1;
        let y: word = 2;
        let observed: word = 0;
        match (x, y) {
            case (y, x) {
                observed = x;
            }
        }
        return uint256(primAddWord(x, observed));
    }

    // #[(42) -> 8]
    function runtimeLocal(value: uint256) public returns (uint256) {
        let value: word = Typedef.rep(value);
        let x: word = 7;
        let z: word = 0;
        match (z) {
            case (_) {
                let x: word = value;
                assembly {
                    x := add(x, 1)
                }
            }
        }
        return uint256(primAddWord(x, 1));
    }

    // #[(42) -> 142]
    function runtimePatternAssignment(value: uint256) public returns (uint256) {
        let value: word = Typedef.rep(value);
        let x: word = 7;
        let y: word = 2;
        let observed: word = 0;
        match (x, y) {
            case (y, x) {
                x = value;
                assembly {
                    y := add(x, 100)
                }
                observed = y;
            }
        }
        if (eqWord(x, 7)) {
            return uint256(observed);
        } else {
            return uint256(0);
        }
    }

    // #[(42) -> 49]
    function writesBeforeShadow(value: uint256) public returns (uint256) {
        let value: word = Typedef.rep(value);
        let x: word = 1;
        let z: word = 0;
        match (z) {
            case (_) {
                x = value;
                let x: word = 7;
                match ((x, z)) {
                    case ((z, x)) {
                        x = 100;
                    }
                }
            }
        }
        {
            let x: word = 0;
            assembly {
                x := calldatasize()
            }
        }
        return uint256(primAddWord(x, 7));
    }

    // #[() -> 6]
    function nestedBindings() public returns (uint256) {
        let x: word = 1;
        let y: word = 2;
        let observed: word = 0;
        match (Pair(x, y)) {
            case (Pair(y, x)) {
                match ((x, y)) {
                    case ((y, x)) {
                        observed = primAddWord(x, y);
                    }
                }
                observed = primAddWord(observed, x);
            }
        }
        return uint256(primAddWord(x, observed));
    }

    // #[() -> 2]
    function earlyReturn() public returns (uint256) {
        let x: word = 1;
        let y: word = 2;
        match (x, y) {
            case (y, x) {
                return uint256(x);
            }
        }
        return uint256(x);
    }

    // #[() -> 8]
    function continueArm() public returns (uint256) {
        let sum = uint256(0);
        for (let i = uint256(0); i < uint256(5); i = i + uint256(1)) {
            match (uint256(0)) {
                case (_) {
                    if (i == uint256(2)) {
                        continue;
                    } else {}
                    sum = sum + i;
                }
            }
        }
        return sum;
    }

    // #[() -> 1]
    function breakArm() public returns (uint256) {
        let sum = uint256(0);
        for (let i = uint256(0); i < uint256(5); i = i + uint256(1)) {
            match (uint256(0)) {
                case (_) {
                    if (i == uint256(2)) {
                        break;
                    } else {}
                    sum = sum + i;
                }
            }
        }
        return sum;
    }

    // #[() -> 8]
    function differentlyTypedLocal() public returns (uint256) {
        let x: word = 7;
        {
            let x: bool = true;
        }
        assembly {
            x := add(x, 1)
        }
        return uint256(x);
    }

    // #[() -> 8]
    function differentlyTypedPattern() public returns (uint256) {
        let x: word = 7;
        let flag: bool = true;
        match (flag) {
            case (x) {}
        }
        assembly {
            x := add(x, 1)
        }
        return uint256(x);
    }

    // The branch-local shadow does not capture the later outer assignment.
    // #[(true) -> 100]
    // #[(false) -> 100]
    function conditionalDeclaration(flag: bool) public returns (uint256) {
        let x: word = 7;
        {
            if (flag) {
                let x: word = 9;
            } else {}
            x = 100;
        }
        return uint256(x);
    }

    // #[(17) -> 34]
    function closureBeforeTypedShadow(value: uint256) public returns (uint256) {
        let n: word = Typedef.rep(value);
        let observed: word = 0;
        let get: function () returns (word) = lam () -> word {
            let v: word;
            assembly {
                v := n
            }
            return v;
        };
        match ((true, get)) {
            case ((n, get)) {
                observed = get();
            }
        }
        return uint256(primAddWord(n, observed));
    }

    // #[(0) -> 8]
    // #[(42) -> 8]
    function dynamicTypedPattern(value: uint256) public returns (uint256) {
        let x: bool = true;
        let observed: word = 0;
        let runtime: word = Typedef.rep(value);
        match (runtime) {
            case (x) {
                assembly {
                    x := 1
                }
                observed = x;
            }
        }
        if (x) {
            return uint256(primAddWord(observed, 7));
        } else {
            return uint256(0);
        }
    }
}
