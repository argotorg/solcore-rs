pragma solcore;

import * from std;
import * from std.dispatch;

function identity(value: word) returns (word) {
    return value;
}

contract ControlFlowLocalLifetimes {
    // #[(true) -> 7]
    // #[(false) -> 7]
    function foldedPatternAssembly(flag: bool) public returns (uint256) {
        let zero: word = 0;
        match ((identity, zero)) {
            case (bundle) {
                if (flag) {
                    let bundle: word = 42;
                } else {}
                match (bundle) {
                    case ((apply, value)) {
                        let result: word;
                        assembly {
                            result := add(value, 7)
                        }
                        return uint256(apply(result));
                    }
                }
            }
        }
    }

    // #[(true) -> 100]
    // #[(false) -> 100]
    function branchAssemblyReadWrite(flag: bool) public returns (uint256) {
        let x: word = 99;
        if (flag) {
            let x: word = 42;
        } else {}
        assembly {
            x := add(x, 1)
        }
        return uint256(x);
    }

    // #[(true) -> 2]
    // #[(false) -> 2]
    function nestedBranchClosure(flag: bool) public returns (uint256) {
        let x: word = 0;
        if (flag) {
            if (flag) {
                let x: word = 7;
            } else {}
        } else {}
        let get: function () returns (word) = lam () -> word {
            let result: word;
            assembly {
                result := add(x, 2)
            }
            return result;
        };
        return uint256(get());
    }

    // #[(0) -> 0]
    // #[(1) -> 7]
    // #[(3) -> 9]
    function loopBodyClosure(count: uint256) public returns (uint256) {
        let total: word = 0;
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            let x: word = primAddWord(Typedef.rep(i), 7);
            total = x;
        }
        let get: function () returns (word) = lam () -> word {
            let result: word;
            assembly {
                result := total
            }
            return result;
        };
        return uint256(get());
    }

    // #[(0) -> 1]
    // #[(1) -> 11]
    // #[(3) -> 13]
    function nestedLoopBranch(count: uint256) public returns (uint256) {
        let total: word = 0;
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            if (i < uint256(2)) {
                let x: word = primAddWord(Typedef.rep(i), 10);
                total = x;
            } else {
                let x: word = 12;
                total = x;
            }
        }
        assembly {
            total := add(total, 1)
        }
        return uint256(total);
    }

    // #[(0) -> 100]
    // #[(1) -> 100]
    // #[(3) -> 100]
    function loopBodyShadow(count: uint256) public returns (uint256) {
        let i: uint256 = uint256(100);
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            let i: uint256 = i;
        }
        return i;
    }

    // #[(0) -> 7]
    // #[(1) -> 77]
    // #[(3) -> 217]
    function loopPostDeclaration(count: uint256) public returns (uint256) {
        let x: word = 7;
        let observed: word = 0;
        for (let i: uint256 = uint256(0); i < count;
            let x: word = primAddWord(Typedef.rep(i), 1), i = i + uint256(1)) {
            assembly {
                observed := add(observed, x)
            }
        }
        assembly {
            observed := add(mul(observed, 10), x)
        }
        return uint256(observed);
    }

    // #[(0) -> 0]
    // #[(1) -> 7]
    // #[(4) -> 8]
    function loopEarlyExits(count: uint256) public returns (uint256) {
        let result: word = 0;
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            let x: word = primAddWord(Typedef.rep(i), 7);
            result = x;
            if (eqWord(Typedef.rep(i), 1)) {
                break;
            }
            continue;
        }
        return uint256(result);
    }

    // #[(0) -> 0]
    // #[(3) -> 1]
    function loopDefaultInitialization(count: uint256) public returns (uint256) {
        let result: word = 0;
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            let x: word;
            assembly {
                x := add(x, 1)
            }
            result = x;
        }
        return uint256(result);
    }

    // #[(0) -> 0]
    // #[(3) -> 1]
    function nestedDefaultInitialization(count: uint256) public returns (uint256) {
        let result: word = 0;
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            if (i < count) {
                let x: word;
                assembly {
                    x := add(x, 1)
                }
                result = x;
            }
        }
        return uint256(result);
    }

    // #[(0) -> 0]
    // #[(3) -> 0]
    function loopCompositeDefault(count: uint256) public returns (uint256) {
        let result: word = 0;
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            let value: (word, word);
            match (value) {
                case ((a, b)) {
                    result = primAddWord(a, b);
                }
            }
            value = (7, 8);
        }
        return uint256(result);
    }

    // #[(true) -> 21]
    // #[(false) -> 14]
    function assemblyLexicalScopes(flag: bool) public returns (uint256) {
        let x: word = 0;
        if (flag) {
            x = 7;
        } else {}
        let result: word = 0;
        assembly {
            {
                let x := add(x, 1)
                result := x
            }
            for {
                let x := 0
            } lt(x, 2) {
                x := add(x, 1)
            } {
                result := add(result, x)
            }
            if x {
                let x := 90
                result := add(result, 1)
            }
            switch x
            case 7 {
                let x := 3
                result := add(result, x)
            }
            default {
                let x := 4
                result := add(result, x)
            }
            function x(value) -> result {
                result := add(value, 2)
            }
            result := add(result, x(6))
            x := add(x, 1)
        }
        return uint256(result);
    }

    // #[(true) -> 2]
    // #[(false) -> 2]
    function repeatedClosure(flag: bool) public returns (uint256) {
        let read: function (bool) returns (word) = lam (flag: bool) -> word {
            let x: word = 0;
            if (flag) {
                let x: word = 7;
            } else {}
            assembly {
                x := add(x, 1)
            }
            return x;
        };
        return uint256(primAddWord(read(flag), read(false)));
    }

    // #[() -> 1]
    function constantFalseDeclaration() public returns (uint256) {
        let x: word = 1;
        if (false) {
            let x: word = 7;
        } else {}
        return uint256(x);
    }

    // #[() -> 1]
    function constantTrueElseShadow() public returns (uint256) {
        let x: word = 1;
        if (true) {
            let x: word = 42;
        } else {
            let x: word = 7;
        }
        return uint256(x);
    }

    // #[(true) -> 1]
    // #[(false) -> 1]
    function runtimeElseShadow(flag: bool) public returns (uint256) {
        let x: word = 1;
        if (flag) {
            let x: word = 42;
        } else {
            let x: word = 7;
        }
        return uint256(x);
    }

    // #[(true) -> 2]
    // #[(false) -> 2]
    function explicitScopeDoesNotEscape(flag: bool) public returns (uint256) {
        let x: word = 1;
        {
            if (flag) {
                let x: word = 7;
            } else {}
            assembly {
                x := add(x, 1)
            }
        }
        match (flag) {
            case (_) {
                for (; false;) {
                    let x: word = 9;
                }
            }
        }
        return uint256(x);
    }
}
