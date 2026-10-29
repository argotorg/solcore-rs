pragma solcore;

import * from std;
import * from std.dispatch;

contract LoopPostReachability {
    // #[(0) -> 99]
    // #[(3) -> 99]
    function coreBreak(count: uint256) public returns (uint256) {
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            break;
        }
        return uint256(99);
    }

    // #[(0) -> 99]
    // #[(3) -> 0]
    function coreReturn(count: uint256) public returns (uint256) {
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            return i;
        }
        return uint256(99);
    }

    // A terminated body still reaches post through a nested continue.
    // #[(0) -> 0]
    // #[(1) -> 1]
    // #[(5) -> 2]
    function coreMatch(count: uint256) public returns (uint256) {
        let i: uint256 = uint256(0);
        for (; i < count; i = i + uint256(1)) {
            match (i < uint256(2)) {
                case true {
                    continue;
                }
                case false {
                    break;
                }
            }
        }
        return i;
    }

    // #[(0) -> 99]
    // #[(1) -> 99]
    // #[(5) -> 2]
    function coreContinueOrReturn(count: uint256) public returns (uint256) {
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            if (i < uint256(2)) {
                continue;
            }
            return i;
        }
        return uint256(99);
    }

    // Inner-loop continues must not make the outer post reachable.
    // #[(0) -> 0]
    // #[(3) -> 3]
    function nestedLoops(count: uint256) public returns (uint256) {
        let seen: uint256 = uint256(0);
        for (let outer: uint256 = uint256(0); outer < count; outer = outer + uint256(1)) {
            for (let inner: uint256 = uint256(0); inner < count; inner = inner + uint256(1)) {
                seen = seen + uint256(1);
                continue;
            }
            break;
        }
        return seen;
    }

    // #[(0) -> 99]
    // #[(3) -> 99]
    function yulBreak(count: uint256) public returns (uint256) {
        let n: word = Typedef.rep(count);
        assembly {
            for {
                let i := 0
            } lt(i, n) {
                i := add(i, 1)
            } {
                break
            }
        }
        return uint256(99);
    }

    // #[(0) -> 99]
    // #[(3) -> 0]
    function yulReturn(count: uint256) public returns (uint256) {
        let n: word = Typedef.rep(count);
        assembly {
            for {
                let i := 0
            } lt(i, n) {
                i := add(i, 1)
            } {
                mstore(0, i)
                return (0, 32)
            }
        }
        return uint256(99);
    }

    // An exhaustive switch has no fallthrough, but continue still reaches post.
    // #[(0) -> 0]
    // #[(1) -> 1]
    // #[(5) -> 2]
    function yulSwitchContinue(count: uint256) public returns (uint256) {
        let n: word = Typedef.rep(count);
        let result: word;
        assembly {
            let i := 0
            for {} lt(i, n) {
                i := add(i, 1)
            } {
                switch i
                case 0 {
                    continue
                }
                case 1 {
                    continue
                }
                default {
                    break
                }
            }
            result := i
        }
        return uint256(result);
    }

    // #[(0) -> 0]
    // #[(3) -> 3]
    function yulNested(count: uint256) public returns (uint256) {
        let n: word = Typedef.rep(count);
        let result: word;
        assembly {
            for {
                let outer := 0
            } lt(outer, n) {
                outer := add(outer, 1)
            } {
                for {
                    let inner := 0
                } lt(inner, n) {
                    inner := add(inner, 1)
                } {
                    result := add(result, 1)
                    continue
                }
                break
            }
        }
        return uint256(result);
    }

    // #[(0) -> 0]
    // #[(1) -> 1]
    // #[(5) -> 2]
    function yulContinue(count: uint256) public returns (uint256) {
        let n: word = Typedef.rep(count);
        let result: word;
        assembly {
            let i := 0
            for {} lt(i, n) {
                i := add(i, 1)
            } {
                if lt(i, 2) {
                    continue
                }
                break
            }
            result := i
        }
        return uint256(result);
    }

    // Every switch arm exits the loop; its merge and post are both dead.
    // #[(0) -> 99]
    // #[(1) -> 99]
    // #[(2) -> revert(0x)]
    function yulSwitch(count: uint256) public returns (uint256) {
        let n: word = Typedef.rep(count);
        assembly {
            for {
                let i := 0
            } lt(i, n) {
                i := add(i, 1)
            } {
                switch n
                case 1 {
                    break
                }
                default {
                    revert(0, 0)
                }
            }
        }
        return uint256(99);
    }

    // #[(0) -> 99]
    // #[(3) -> 0]
    function yulLeave(count: uint256) public returns (uint256) {
        let n: word = Typedef.rep(count);
        let result: word;
        assembly {
            function first(n) -> result {
                result := 99
                for {
                    let i := 0
                } lt(i, n) {
                    i := add(i, 1)
                } {
                    result := i
                    leave
                }
            }
            result := first(n)
        }
        return uint256(result);
    }

    // #[(0) -> 99]
    // #[(1) -> revert(0x)]
    function coreRevert(count: uint256) public returns (uint256) {
        for (let i: uint256 = uint256(0); i < count; i = i + uint256(1)) {
            assembly {
                revert(0, 0)
            }
        }
        return uint256(99);
    }

    // #[(0) -> 99]
    // #[(1) -> revert(0x)]
    function yulPostRevert(count: uint256) public returns (uint256) {
        let n: word = Typedef.rep(count);
        assembly {
            for {
                let i := 0
            } lt(i, n) {
                revert(0, 0)
            } {
                continue
            }
        }
        return uint256(99);
    }
}

//~ sonatina: file-url-imports
//~ sonatina: compile O0
