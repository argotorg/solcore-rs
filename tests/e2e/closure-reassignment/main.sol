pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch

import * from std;
import * from std.dispatch;

// Closure variables assigned in a block, inside a lambda, or under run-time
// control flow (issues #160 and #165). Captures are by value at closure
// creation; calls through a variable select among the closures that can reach
// them.

enum Choice {
    First,
    Second,
    Third
}

enum Box<a> {
    Box(a)
}

function twice(f: function (word) returns (word), x: word) returns (word) {
    return f(f(x));
}

contract ClosureReassignment {
    count: uint256;

    function makeCounted(step: word) returns (function (word) returns (word)) {
        count = count + uint256(1);
        let base: word = step + Typedef.rep(count);
        return lam (a: word) -> word {
            return a + base;
        };
    }

    // #[(1) -> 0]
    function lambdaReassign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + x;
        };
        let h: function (word) returns (word) = lam (a: word) -> word {
            let g: function (word) returns (word) = lam (b: word) -> word {
                return b;
            };
            g = f;
            return 0;
        };
        return uint256(h(x));
    }

    // #[(1) -> 0]
    // #[(5) -> 0]
    function lambdaReassignCalled(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + x;
        };
        let h: function (word) returns (word) = lam (a: word) -> word {
            let g: function (word) returns (word) = lam (b: word) -> word {
                return b;
            };
            g = f;
            return g(a) - a - x;
        };
        return uint256(h(x));
    }

    // #[(1) -> 2]
    // #[(5) -> 10]
    function lambdaReassignInBlock(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + x;
        };
        let h: function (word) returns (word) = lam (a: word) -> word {
            let g: function (word) returns (word) = lam (b: word) -> word {
                return b;
            };
            {
                g = f;
            }
            return g(a);
        };
        return uint256(h(x));
    }

    // #[(1) -> 2]
    // #[(5) -> 10]
    function lambdaReassignCaptured(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + x;
        };
        let h: function (word) returns (word) = lam (a: word) -> word {
            let g: function (word) returns (word) = lam (b: word) -> word {
                return b;
            };
            g = f;
            let k: function () returns (word) = lam () -> word {
                return g(a);
            };
            return k();
        };
        return uint256(h(x));
    }

    // #[(1) -> 2]
    // #[(5) -> 10]
    function lambdaReassignNested(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + x;
        };
        let h: function (word) returns (word) = lam (a: word) -> word {
            let m: function (word) returns (word) = lam (c: word) -> word {
                let g: function (word) returns (word) = lam (b: word) -> word {
                    return b;
                };
                g = f;
                return g(c);
            };
            return m(a);
        };
        return uint256(h(x));
    }

    // #[(1) -> 2]
    // #[(7) -> 7]
    function lambdaBranchReassign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + x;
        };
        let h: function (word) returns (word) = lam (a: word) -> word {
            let g: function (word) returns (word) = lam (b: word) -> word {
                return b;
            };
            if (a < 5) {
                g = f;
            }
            return g(a);
        };
        return uint256(h(x));
    }

    // A local closure that shadows an outer one keeps its own value.
    // #[(1) -> 3]
    // #[(7) -> 8]
    function lambdaLocalShadow(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 100;
        };
        let h: function (word) returns (word) = lam (c: word) -> word {
            let f: function (word) returns (word) = lam (a: word) -> word {
                return a + 1;
            };
            if (c < 5) {
                f = lam (a: word) -> word {
                    return a + 2;
                };
            }
            return f(c);
        };
        return uint256(h(x));
    }

    // #[(0) -> 1]
    // #[(3) -> 5]
    function lambdaLoopReassign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let h: function (word) returns (word) = lam (n: word) -> word {
            let f: function (word) returns (word) = lam (a: word) -> word {
                return a + 1;
            };
            let i: word = 0;
            while (i < n) {
                f = lam (a: word) -> word {
                    return a + 2;
                };
                i = i + 1;
            }
            return f(n);
        };
        return uint256(h(x));
    }

    // #[(1) -> 2]
    // #[(5) -> 10]
    function blockReassign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let g: function (word) returns (word) = lam (b: word) -> word {
            return b;
        };
        {
            g = lam (a: word) -> word {
                return a + x;
            };
        }
        x = 7;
        return uint256(g(Typedef.rep(input)));
    }

    // #[(1) -> 3]
    // #[(200) -> 201]
    function branchReassign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        if (x < 100) {
            f = lam (a: word) -> word {
                return a + 2;
            };
        }
        return uint256(f(x));
    }

    // #[(1) -> 3]
    // #[(200) -> 230]
    function branchElse(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        if (x < 100) {
            f = lam (a: word) -> word {
                return a + 2;
            };
        } else {
            f = lam (a: word) -> word {
                return a + 30;
            };
        }
        return uint256(f(x));
    }

    // #[(1) -> 3]
    // #[(200) -> 230]
    function assignedOnBothPaths(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word);
        if (x < 100) {
            f = lam (a: word) -> word {
                return a + 2;
            };
        } else {
            f = lam (a: word) -> word {
                return a + 30;
            };
        }
        return uint256(f(x));
    }

    // #[(0) -> 10]
    // #[(1) -> 21]
    // #[(2) -> 32]
    function matchArms(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let c: Choice = Choice.Third;
        if (x == 0) {
            c = Choice.First;
        } else {
            if (x == 1) {
                c = Choice.Second;
            }
        }
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 10;
        };
        match (c) {
            case Choice.First {}
            case Choice.Second {
                f = lam (a: word) -> word {
                    return a + 20;
                };
            }
            case Choice.Third {
                f = lam (a: word) -> word {
                    return a + 30;
                };
            }
        }
        return uint256(f(x));
    }

    // #[(1) -> 3]
    // #[(60) -> 63]
    // #[(200) -> 201]
    function nestedBranches(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        if (x < 100) {
            f = lam (a: word) -> word {
                return a + 2;
            };
            if (x > 50) {
                f = lam (a: word) -> word {
                    return a + 3;
                };
            }
        }
        return uint256(f(x));
    }

    // #[(1, 7) -> 2]
    // #[(200, 7) -> 208]
    // #[(60, 7) -> 3]
    function differentCaptures(input: uint256, k: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let y: word = Typedef.rep(k);
        let flag: bool = x > 50;
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + y + 1;
        };
        if (x < 100) {
            f = lam (a: word) -> word {
                if (flag) {
                    return 3;
                }
                return a + 1;
            };
        }
        y = 0;
        return uint256(f(x));
    }

    // #[(1) -> 3]
    // #[(200) -> 202]
    function sameLambda(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let g: function (word) returns (word) = lam (a: word) -> word {
            return a + 2;
        };
        let f: function (word) returns (word) = g;
        if (x < 100) {
            f = g;
        }
        return uint256(f(x));
    }

    // #[(1, 5) -> 7]
    // #[(200, 5) -> 205]
    function sameLambdaCapturing(input: uint256, k: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let y: word = Typedef.rep(k);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + y;
        };
        if (x < 100) {
            y = y + 1;
            f = lam (a: word) -> word {
                return a + y;
            };
        }
        y = 50;
        return uint256(f(x));
    }

    // #[(1) -> 13]
    // #[(200) -> 201]
    function useBeforeReassign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        let before: word = 0;
        if (x < 100) {
            before = f(x);
            f = lam (a: word) -> word {
                return a + 10;
            };
        }
        return uint256(before + f(x));
    }

    // #[(1) -> 3]
    // #[(200) -> 7]
    function callInLaterBranch(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        if (x < 100) {
            f = lam (a: word) -> word {
                return a + 2;
            };
        }
        let result: word = 7;
        if (x < 150) {
            result = f(x);
        }
        return uint256(result);
    }

    // #[(1) -> 4]
    // #[(200) -> 600]
    function callInLaterLoop(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 100;
        };
        if (x < 100) {
            f = lam (a: word) -> word {
                return a + 1;
            };
        }
        let total: word = 0;
        let i: word = 0;
        while (i < 2) {
            total = total + f(x);
            i = i + 1;
        }
        return uint256(total);
    }

    // #[(1) -> 3]
    // #[(200) -> 202]
    function passToHigherOrder(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        if (x < 100) {
            f = lam (a: word) -> word {
                return a + x;
            };
        }
        return uint256(twice(f, x));
    }

    // #[(1) -> 4]
    // #[(200) -> 202]
    function captureJoined(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        if (x < 100) {
            f = lam (a: word) -> word {
                return a + 2;
            };
        }
        let k: function () returns (word) = lam () -> word {
            return f(x) + 1;
        };
        return uint256(k());
    }

    // #[(1) -> 8]
    // #[(200) -> 207]
    function reassignAfterJoin(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        if (x < 100) {
            f = lam (a: word) -> word {
                return a + 2;
            };
        }
        f = lam (a: word) -> word {
            return a + 7;
        };
        return uint256(f(x));
    }

    // #[(1) -> 4]
    // #[(50) -> 52]
    // #[(200) -> 201]
    function joinTwice(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        if (x < 100) {
            f = lam (a: word) -> word {
                return a + 2;
            };
        }
        if (x < 10) {
            f = lam (a: word) -> word {
                return a + 3;
            };
        }
        return uint256(f(x));
    }

    // #[(1) -> 3]
    // #[(200) -> 201]
    function captureOfCapture(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let g: function (word) returns (word) = lam (a: word) -> word {
            return a + x;
        };
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        if (x < 100) {
            f = lam (a: word) -> word {
                return g(a) + 1;
            };
        }
        return uint256(f(x));
    }

    // #[(1) -> 11]
    // #[(200) -> 1]
    function nullaryReassign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function () returns (word) = lam () -> word {
            return 1;
        };
        if (x < 100) {
            f = lam () -> word {
                return x + 10;
            };
        }
        return uint256(f());
    }

    // #[(1) -> 3]
    // #[(200) -> 201]
    function twoParamReassign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word, word) returns (word) = lam (a: word, b: word) -> word {
            return a + b;
        };
        if (x < 100) {
            f = lam (a: word, b: word) -> word {
                return a + b + x;
            };
        }
        return uint256(f(x, 1));
    }

    // #[(1) -> 3]
    // #[(200) -> 201]
    function tupleReassign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let t: (function (word) returns (word), word) = (lam (a: word) -> word {
                return a + 1;
            }, 0);
        if (x < 100) {
            t = (lam (a: word) -> word {
                    return a + 2;
                }, 1);
        }
        match (t) {
            case (f, offset) {
                return uint256(f(x) + offset - offset);
            }
        }
    }

    // #[(1) -> 3]
    // #[(200) -> 201]
    function boxReassign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let boxed: Box<function (word) returns (word)> = Box(lam (a: word) -> word {
                return a + 1;
            });
        if (x < 100) {
            boxed = Box(lam (a: word) -> word {
                    return a + 2;
                });
        }
        match (boxed) {
            case (Box(f)) {
                return uint256(f(x));
            }
        }
    }

    // The factory call, and its storage effect, runs once at the assignment.
    // #[(1) -> 3]
    // #[(200) -> 0]
    function effectOnce(input: uint256) public returns (uint256) {
        count = uint256(0);
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a;
        };
        if (x < 100) {
            f = makeCounted(x);
        }
        let first: word = f(0);
        let second: word = f(0);
        return uint256(first + second - first) + count;
    }

    // #[(0) -> 1]
    // #[(3) -> 5]
    function loopAssign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        let i: word = 0;
        while (i < x) {
            f = lam (a: word) -> word {
                return a + 2;
            };
            i = i + 1;
        }
        return uint256(f(x));
    }

    // Each iteration captures the current counter by value.
    // #[(0) -> 100]
    // #[(3) -> 5]
    function loopCapturesCounter(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 100;
        };
        let i: word = 0;
        while (i < x) {
            f = lam (a: word) -> word {
                return a + i;
            };
            i = i + 1;
        }
        return uint256(f(x));
    }

    // #[(0) -> 0]
    // #[(1) -> 11]
    // #[(3) -> 35]
    function loopCallsBeforeAssign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a;
        };
        let total: word = 0;
        let i: word = 0;
        for (; i < x; i = i + 1) {
            total = total + f(10);
            f = lam (a: word) -> word {
                return a + 1;
            };
        }
        return uint256(total + x);
    }

    // #[(3) -> 53]
    // #[(20) -> 11]
    function loopWithBreak(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        let i: word = 0;
        while (i < 10) {
            if (i == x) {
                f = lam (a: word) -> word {
                    return a + 50;
                };
                break;
            }
            i = i + 1;
        }
        return uint256(f(i));
    }

    // #[(0) -> 1]
    // #[(2) -> 12]
    // #[(4) -> 7]
    function loopWithContinue(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        let i: word = 0;
        while (i < x) {
            i = i + 1;
            if (i == 2) {
                f = lam (a: word) -> word {
                    return a + 10;
                };
                continue;
            }
            f = lam (a: word) -> word {
                return a + 3;
            };
        }
        return uint256(f(x));
    }

    // #[(0) -> 1]
    // #[(1) -> 2]
    // #[(2) -> 4]
    function nestedLoops(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        let i: word = 0;
        while (i < x) {
            let j: word = 0;
            while (j < i) {
                f = lam (a: word) -> word {
                    return a + 2;
                };
                j = j + 1;
            }
            i = i + 1;
        }
        return uint256(f(x));
    }

    // #[(0) -> 10]
    // #[(1) -> 21]
    // #[(2) -> 32]
    // #[(5) -> 35]
    function loopAssignsInBranch(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 10;
        };
        let i: word = 0;
        while (i < x) {
            if (i == 0) {
                f = lam (a: word) -> word {
                    return a + 20;
                };
            } else {
                f = lam (a: word) -> word {
                    return a + 30;
                };
            }
            i = i + 1;
            if (i == 3) {
                break;
            }
        }
        return uint256(f(x));
    }

    // The template restates the value that `g` held before the branch.
    // #[(1) -> 2]
    // #[(7) -> 27]
    function lambdaRestatesBeforeBranch(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + x;
        };
        let h: function (word) returns (word) = lam (a: word) -> word {
            let g: function (word) returns (word) = lam (b: word) -> word {
                return b + 10;
            };
            g = lam (b: word) -> word {
                return b + 20;
            };
            if (a < 5) {
                g = f;
            }
            return g(a);
        };
        return uint256(h(x));
    }

    // #[(0) -> 10]
    // #[(3) -> 6]
    function lambdaLoopAssignsCapture(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + x;
        };
        let h: function (word) returns (word) = lam (n: word) -> word {
            let g: function (word) returns (word) = lam (b: word) -> word {
                return b + 10;
            };
            let i: word = 0;
            while (i < n) {
                g = f;
                i = i + 1;
            }
            return g(n);
        };
        return uint256(h(x));
    }

    // A lambda can return the selected closure: its environment is returned.
    // #[(1, 1) -> 3]
    // #[(1, 7) -> 2]
    function lambdaReturnsSelected(input: uint256, choice: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let select: function (word) returns (function (word) returns (word)) = lam (y: word) -> function (word) returns (word) {
            let f: function (word) returns (word) = lam (a: word) -> word {
                return a + 1;
            };
            if (y < 5) {
                f = lam (a: word) -> word {
                    return a + 2;
                };
            }
            return f;
        };
        let g: function (word) returns (word) = select(Typedef.rep(choice));
        return uint256(g(x));
    }

    // #[(0) -> 0]
    // #[(2) -> 7]
    // #[(3) -> 10]
    function loopLocalReassign(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let total: word = 0;
        let i: word = 0;
        while (i < x) {
            let f: function (word) returns (word) = lam (a: word) -> word {
                return a + 1;
            };
            if (i == 1) {
                f = lam (a: word) -> word {
                    return a + 5;
                };
            }
            total = total + f(i);
            i = i + 1;
        }
        return uint256(total);
    }

    // #[(0) -> 1]
    // #[(1) -> 11]
    // #[(2) -> 22]
    function matchInLoop(input: uint256) public returns (uint256) {
        let x: word = Typedef.rep(input);
        let f: function (word) returns (word) = lam (a: word) -> word {
            return a + 1;
        };
        let c: Choice = Choice.First;
        let i: word = 0;
        while (i < x) {
            match (c) {
                case Choice.First {
                    f = lam (a: word) -> word {
                        return a + 10;
                    };
                    c = Choice.Second;
                }
                case Choice.Second {
                    f = lam (a: word) -> word {
                        return a + 20;
                    };
                }
                case Choice.Third {}
            }
            i = i + 1;
        }
        return uint256(f(x));
    }
}
