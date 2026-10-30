pragma solcore;

import * from std;
import * from std.dispatch;

contract ControlFlowCaptureIdentity {
    // #[(true, 3) -> 13]
    // #[(true, 0) -> 7]
    // #[(false, 3) -> 0]
    function branch(flag: bool, value: uint256) public returns (uint256) {
        let x: word = Typedef.rep(value);
        if (flag) {
            let first: function () returns (word) = lam () -> word {
                let result: word;
                assembly {
                    result := x
                }
                return result;
            };
            assembly {
                x := add(x, 7)
            }
            let second: function () returns (word) = lam () -> word {
                let result: word;
                assembly {
                    result := x
                }
                return result;
            };
            return uint256(primAddWord(first(), second()));
        }
        return uint256(0);
    }

    // #[(0, 3) -> 0]
    // #[(1, 3) -> 13]
    // #[(2, 3) -> 27]
    function loop(count: uint256, value: uint256) public returns (uint256) {
        let x: word = Typedef.rep(value);
        let total: word = 0;
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            let first: function () returns (word) = lam () -> word {
                let result: word;
                assembly {
                    result := x
                }
                return result;
            };
            assembly {
                x := add(x, 7)
            }
            let second: function () returns (word) = lam () -> word {
                let result: word;
                assembly {
                    result := x
                }
                return result;
            };
            total = primAddWord(first(), second());
        }
        return uint256(total);
    }

    // #[(true, 1, 3) -> 30]
    // #[(true, 2, 3) -> 72]
    // #[(false, 1, 3) -> 0]
    // #[(true, 0, 3) -> 0]
    function nested(flag: bool, count: uint256, value: uint256) public returns (uint256) {
        let x: word = Typedef.rep(value);
        let total: word = 0;
        if (flag) {
            for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
                let first: function () returns (word) = lam () -> word {
                    let result: word;
                    assembly {
                        result := x
                    }
                    return result;
                };
                assembly {
                    x := add(x, 7)
                }
                {
                    let second: function () returns (word) = lam () -> word {
                        let result: word;
                        assembly {
                            result := x
                        }
                        return result;
                    };
                    assembly {
                        x := add(x, 7)
                    }
                    if (flag) {
                        let third: function () returns (word) = lam () -> word {
                            let result: word;
                            assembly {
                                result := x
                            }
                            return result;
                        };
                        total = primAddWord(primAddWord(first(), second()), third());
                    }
                }
            }
        }
        return uint256(total);
    }
}
