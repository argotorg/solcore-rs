pragma solcore;

import * from std;
import * from std.dispatch;

contract IncrementalReachability {
    // Nested dead merges must not hide a continue edge to the loop post.
    // #[(0) -> 0]
    // #[(1) -> 0]
    // #[(2) -> 1]
    // #[(5) -> 1]
    function nestedSwitch(count: uint256) public returns (uint256) {
        let n: word = Typedef.rep(count);
        let result: word;
        assembly {
            let i := 0
            for {} lt(i, n) {
                i := add(i, 1)
            } {
                switch i
                case 0 {
                    switch n
                    case 1 {
                        break
                    }
                    default {
                        continue
                    }
                }
                default {
                    break
                }
                i := 999
            }
            result := i
        }
        return uint256(result);
    }

    // Core match continuations share the same reachability rules as assembly.
    // #[(0) -> 0]
    // #[(1) -> 0]
    // #[(2) -> 1]
    // #[(5) -> 1]
    function nestedMatch(count: uint256) public returns (uint256) {
        let i: uint256 = uint256(0);
        for (; i < count; i = i + uint256(1)) {
            match (i == uint256(0)) {
                case true {
                    match (count == uint256(1)) {
                        case true {
                            break;
                        }
                        case false {
                            continue;
                        }
                    }
                }
                case false {
                    break;
                }
            }
            i = uint256(999);
        }
        return i;
    }

    // A Yul function has its own reachable entry. Every nested switch arm
    // leaves the callee.
    // #[(0) -> 7]
    // #[(1) -> 11]
    // #[(2) -> 13]
    function nestedLeave(value: uint256) public returns (uint256) {
        let n: word = Typedef.rep(value);
        let result: word;
        assembly {
            function choose(n) -> out {
                switch n
                case 0 {
                    out := 7
                    leave
                }
                default {
                    switch n
                    case 1 {
                        out := 11
                        leave
                    }
                    default {
                        out := 13
                        leave
                    }
                }
                out := 999
            }
            result := choose(n)
        }
        return uint256(result);
    }
}

//~ sonatina: file-url-imports
//~ sonatina: compile O0
//~ sonatina: compile O2
