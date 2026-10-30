pragma solcore;

import * from std;
import * from std.dispatch;

function chooseCapture(value: word) returns (function () returns (word)) {
    let x: word = value;
    if (true) {
        let x: word = primAddWord(x, 10);
        return lam () -> word {
            return x;
        };
    } else {
        return lam () -> word {
            return x;
        };
    }
}

contract LexicalControlFlowScopes {
    // #[() -> 1]
    function reported() public returns (uint256) {
        let x: word = 1;
        if (true) {
            let x: word = 2;
        }
        return uint256(x);
    }

    // #[() -> 1]
    function knownElse() public returns (uint256) {
        let x: word = 1;
        if (false) {
            let x: word = 2;
        } else {
            let x: word = 3;
        }
        return uint256(x);
    }

    // #[(true, 13) -> 13]
    // #[(false, 29) -> 29]
    function runtimeShadow(flag: bool, value: uint256) public returns (uint256) {
        let x: uint256 = value;
        if (flag) {
            let x: bool = false;
        } else {
            let x: word = 3;
        }
        return x;
    }

    // #[(true) -> 71]
    // #[(false) -> 11]
    function independentBranches(flag: bool) public returns (uint256) {
        let x: word = 1;
        let result: word = 0;
        if (flag) {
            let x: word = 7;
            result = x;
        } else {
            result = x;
        }
        return uint256(result * 10 + x);
    }

    // #[(true) -> 9]
    // #[(false) -> 1]
    function outerWriteBeforeShadow(flag: bool) public returns (uint256) {
        let x: word = 1;
        if (flag) {
            x = 9;
            let x: word = 2;
            x = 3;
        }
        return uint256(x);
    }

    // #[() -> 9]
    function knownOuterWriteBeforeShadow() public returns (uint256) {
        let x: word = 1;
        if (true) {
            x = 9;
            let x: word = 2;
        }
        return uint256(x);
    }

    // #[(true) -> 1]
    // #[(false) -> 7]
    function assemblyWrites(flag: bool) public returns (uint256) {
        let x: word = 1;
        if (flag) {
            let x: word = 2;
            assembly {
                x := 9
            }
        } else {
            assembly {
                x := 7
            }
        }
        return uint256(x);
    }

    // #[(0) -> 7]
    // #[(3) -> 7]
    function initializerScope(count: uint256) public returns (uint256) {
        let x: word = 7;
        for (let x: word = 0; uint256(x) < count; x = primAddWord(x, 1)) {
            let x: word = 99;
        }
        return uint256(x);
    }

    // #[(0) -> 7]
    // #[(3) -> 10]
    function bodyScope(count: uint256) public returns (uint256) {
        let x: word = 7;
        let sum: word = 0;
        for (let i = uint256(0); i < count; i = i + uint256(1)) {
            let x: word = Typedef.rep(i);
            sum = primAddWord(sum, x);
        }
        return uint256(primAddWord(sum, x));
    }

    // #[(0) -> 7]
    // #[(3) -> 28]
    function postScope(count: uint256) public returns (uint256) {
        let x: word = 7;
        let sum: word = 0;
        for (let i = uint256(0); i < count; let x: word = 99, i = i + uint256(1)) {
            sum = primAddWord(sum, x);
        }
        return uint256(primAddWord(sum, x));
    }

    // #[(false) -> 7]
    // #[(true) -> 7]
    function whileScope(running: bool) public returns (uint256) {
        let x: word = 7;
        while (running) {
            let x: word = 2;
            running = false;
        }
        return uint256(x);
    }

    // #[(13) -> 23]
    // #[(29) -> 39]
    function returnedCapture(value: uint256) public returns (uint256) {
        let get = chooseCapture(Typedef.rep(value));
        return uint256(get());
    }

    // #[(true, 13) -> 13]
    // #[(false, 29) -> 29]
    function captureAfterBranch(flag: bool, value: uint256) public returns (uint256) {
        let x: uint256 = value;
        if (flag) {
            let x: bool = false;
        }
        let get = lam () -> uint256 {
            return x;
        };
        return get();
    }

    // #[(true) -> 7]
    // #[(false) -> 7]
    function nestedMatch(flag: bool) public returns (uint256) {
        let x: word = 7;
        match (flag) {
            case true {
                let x: word = 8;
                if (flag) {
                    let x: bool = false;
                }
                assembly {
                    x := 9
                }
            }
            case false {
                let x: bool = true;
            }
        }
        return uint256(x);
    }
}
