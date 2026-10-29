pragma solcore;

import * from std;
import * from std.dispatch;

function makeAdder(value: uint256) returns (function (uint256) returns (uint256)) {
    return lam (other: uint256) -> uint256 {
        return value + other;
    };
}

function makeComputedAdder(value: uint256) returns (function (uint256) returns (uint256)) {
    let incremented = value + uint256(1);
    return lam (other: uint256) -> uint256 {
        return incremented + other;
    };
}

function makeCheckedConstant(value: uint256) returns (function () returns (uint256)) {
    let unused = value + uint256(1);
    return lam () -> uint256 {
        return uint256(7);
    };
}

function twice(f: function (uint256) returns (uint256), x: uint256) returns (uint256) {
    return f(f(x));
}

function forward(f: function (uint256) returns (uint256), x: uint256) returns (uint256) {
    return twice(f, x);
}

function repeat(f: function (uint256) returns (uint256), x: uint256, n: uint256) returns (uint256) {
    if (n == uint256(0)) {
        return x;
    }
    return repeat(f, f(x), n - uint256(1));
}

function increment(x: uint256) returns (uint256) {
    return x + uint256(1);
}

function run(f: function () returns (uint256)) returns (uint256) {
    return f();
}

function apply2(f: function (uint256, uint256) returns (uint256), x: uint256, y: uint256) returns (uint256) {
    return f(x, y);
}

function applyWord(f: function (word) returns (word), x: word) returns (word) {
    return f(x);
}

function applyGeneric<a>(f: function (a) returns (a), x: a) returns (a) {
    return f(x);
}

function forwardValue<a>(value: a) returns (a) {
    return value;
}

contract ClosureParameters {
    count: uint256;
    incremented: uint256;
    locked: bool;

    // #[() -> 42]
    function inferred() public returns (uint256) {
        let f = lam (x: word) -> word {
            return x;
        };
        return uint256(applyWord(f, 42));
    }

    // #[(10, 22) -> 42]
    // #[(7, 3) -> 17]
    function inferredCaptured(amount: uint256, x: uint256) public returns (uint256) {
        let f = lam (v: uint256) -> uint256 {
            return v + amount;
        };
        amount = uint256(99);
        return forward(f, x);
    }

    // #[(10, 22) -> 42]
    // #[(7, 3) -> 17]
    function inferredAlias(amount: uint256, x: uint256) public returns (uint256) {
        let f = lam (v: uint256) -> uint256 {
            return v + amount;
        };
        let g = f;
        return twice(g, x);
    }

    // #[(10, 32) -> 42]
    // #[(7, 3) -> 10]
    function inferredGeneric(amount: uint256, x: uint256) public returns (uint256) {
        let f = lam (v: uint256) -> uint256 {
            return v + amount;
        };
        return applyGeneric(f, x);
    }

    // #[(40) -> 42]
    // #[(3) -> 5]
    function inferredGenericResult(x: uint256) public returns (uint256) {
        let f = lam (y: uint256) -> uint256 {
            return y + x;
        };
        let callback: function (uint256) returns (uint256) = forwardValue(f);
        return callback(uint256(2));
    }

    // #[(40) -> 42]
    // #[(3) -> 5]
    function inferredIndirectResult(x: uint256) public returns (uint256) {
        let f = lam (y: uint256) -> uint256 {
            return y + x;
        };
        let factory = lam () {
            return f;
        };
        let callback: function (uint256) returns (uint256) = factory();
        return callback(uint256(2));
    }

    // #[(40) -> 42]
    // #[(3) -> 5]
    function inferredNullary(x: uint256) public returns (uint256) {
        let f = lam () -> uint256 {
            return x + uint256(2);
        };
        return run(f);
    }

    // #[(10, 20, 12) -> 42]
    // #[(3, 4, 5) -> 12]
    function inferredMultipleArgs(amount: uint256, x: uint256, y: uint256) public returns (uint256) {
        let f = lam (a: uint256, b: uint256) -> uint256 {
            return a + b + amount;
        };
        return apply2(f, x, y);
    }

    // #[(0) -> 42]
    // #[(8) -> 50]
    function shifted(x: uint256) public returns (uint256) {
        let f = makeAdder(uint256(10));
        let base = f(uint256(32));
        return base + x;
    }

    // #[() -> 42]
    function fixed() public returns (uint256) {
        return twice(lam (v: uint256) -> uint256 {
                return v + uint256(10);
            }, uint256(22));
    }

    // #[(0) -> 20]
    // #[(22) -> 42]
    function addTwenty(x: uint256) public returns (uint256) {
        return twice(lam (v: uint256) -> uint256 {
                return v + uint256(10);
            }, x);
    }

    // #[(22) -> 42]
    function factory(x: uint256) public returns (uint256) {
        return twice(makeAdder(uint256(10)), x);
    }

    // The factory argument is a runtime value and the closure body performs a
    // checked addition, so the capture must survive inlining makeAdder.
    // #[(5, 37) -> 42]
    // #[(1, 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function runtimeFactory(step: uint256, x: uint256) public returns (uint256) {
        let f = makeAdder(step);
        return f(x);
    }

    // #[(4, 37) -> 42]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function computedFactory(step: uint256, x: uint256) public returns (uint256) {
        let f = makeComputedAdder(step);
        return f(x);
    }

    // #[(4) -> 6]
    function factoryLocalShadowsStorage(step: uint256) public returns (uint256) {
        incremented = uint256(100);
        let f = makeComputedAdder(step);
        return f(uint256(1));
    }

    function makeAdderWithStorage(step: uint256) returns (function (uint256) returns (uint256)) {
        let incremented = step + uint256(1);
        return lam (other: uint256) -> uint256 {
            return incremented + count + other;
        };
    }

    function makeAdderWithExpiredShadow(step: uint256) returns (function (uint256) returns (uint256)) {
        {
            let count: bool = true;
        }
        let incremented = step + uint256(1);
        return lam (other: uint256) -> uint256 {
            return incremented + count + other;
        };
    }

    function changeFactoryStorage() {
        count = uint256(20);
    }

    // #[(4) -> 26]
    function factoryCallerShadowsStorage(step: uint256) public returns (uint256) {
        count = uint256(10);
        let count: uint256 = uint256(100);
        let f = makeAdderWithStorage(step);
        changeFactoryStorage();
        return f(uint256(1));
    }

    // #[(4) -> 26]
    function factoryExpiredShadow(step: uint256) public returns (uint256) {
        count = uint256(10);
        let f = makeAdderWithExpiredShadow(step);
        changeFactoryStorage();
        return f(uint256(1));
    }

    // #[(4) -> 1]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff) -> revert(0x4e487b710000000000000000000000000000000000000000000000000000000000000011)]
    function unusedComputedFactory(step: uint256) public returns (uint256) {
        let unused = makeCheckedConstant(step);
        return uint256(1);
    }

    function makeCountedAdder(step: uint256) returns (function (uint256) returns (uint256)) {
        count = count + uint256(1);
        let first = step + count;
        count = count + uint256(1);
        let second = step + count;
        return lam (other: uint256) -> uint256 {
            return first + second + other;
        };
    }

    // #[(3) -> 30]
    function factoryEffectOrder(step: uint256) public returns (uint256) {
        count = uint256(0);
        let f = makeCountedAdder(step);
        count = count + uint256(10);
        return f(uint256(0)) + f(uint256(0)) + count;
    }

    // #[(3) -> 2]
    function unusedFactoryEffects(step: uint256) public returns (uint256) {
        count = uint256(0);
        let unused = makeCountedAdder(step);
        return count;
    }

    // #[(3) -> 2]
    function discardedFactoryEffects(step: uint256) public returns (uint256) {
        count = uint256(0);
        makeCountedAdder(step);
        return count;
    }

    // #[(3) -> 11]
    function reassignedFactoryEffects(step: uint256) public returns (uint256) {
        count = uint256(0);
        let f = makeAdder(step);
        f = makeCountedAdder(step);
        return f(uint256(0)) + count;
    }

    // #[(40) -> 42]
    function shadowed(x: uint256) public returns (uint256) {
        let v = uint256(99);
        return twice(lam (v: uint256) -> uint256 {
                return v + uint256(1);
            }, x);
    }

    // #[(10, 22) -> 42]
    // #[(7, 3) -> 17]
    function captured(amount: uint256, x: uint256) public returns (uint256) {
        return forward(lam (v: uint256) -> uint256 {
                return v + amount;
            }, x);
    }

    // #[(10, 22) -> 42]
    function capturedFieldName(count: uint256, x: uint256) public returns (uint256) {
        return twice(lam (v: uint256) -> uint256 {
                return v + count;
            }, x);
    }

    // #[(21) -> 42]
    function capturedBeforeAssignment(x: uint256) public returns (uint256) {
        let f: function (uint256) returns (uint256) = lam (v: uint256) -> uint256 {
            return v + x;
        };
        x = uint256(99);
        return twice(f, uint256(0));
    }

    // #[(10) -> 12]
    function named(x: uint256) public returns (uint256) {
        return twice(increment, x);
    }

    // #[(3, 5, 4) -> 17]
    // #[(8, 2, 0) -> 2]
    function recursive(amount: uint256, x: uint256, n: uint256) public returns (uint256) {
        return repeat(lam (v: uint256) -> uint256 {
                return v + amount;
            }, x, n);
    }

    function withLock(f: function (uint256) returns (uint256), x: uint256) returns (uint256) {
        locked = true;
        let result = f(x);
        locked = false;
        return result;
    }

    function next() returns (uint256) {
        count = count + uint256(1);
        return count;
    }

    // #[(40) -> 42]
    function wrapped(x: uint256) public returns (uint256) {
        count = uint256(0);
        let result = withLock(lam (v: uint256) -> uint256 {
                require(locked, Error(0x1234));
                return v + x;
            }, next());
        require(!locked, Error(0x5678));
        return result + count;
    }

    // #[() -> 21]
    function ordered() public returns (uint256) {
        count = uint256(0);
        let result = twice(lam (v: uint256) -> uint256 {
                count = count + uint256(1);
                return v + count;
            }, next());
        return result + count * uint256(5);
    }

    // #[() -> 12]
    function ignoredArgument() public returns (uint256) {
        count = uint256(0);
        let result = twice(lam (v: uint256) -> uint256 {
                return uint256(11);
            }, next());
        return result + count;
    }

    // #[() -> 13]
    function constantBodyWithEffects() public returns (uint256) {
        count = uint256(0);
        let result = twice(lam (v: uint256) -> uint256 {
                next();
                return uint256(11);
            }, uint256(0));
        return result + count;
    }

    // #[(40) -> 42]
    function nullary(x: uint256) public returns (uint256) {
        return run(lam () -> uint256 {
                return x + uint256(2);
            });
    }

    // #[(10, 20, 12) -> 42]
    function multipleArgs(amount: uint256, x: uint256, y: uint256) public returns (uint256) {
        return apply2(lam (a: uint256, b: uint256) -> uint256 {
                return a + b + amount;
            }, x, y);
    }

    // #[(40, 2) -> 42]
    function assemblyCapture(x: uint256, amount: uint256) public returns (uint256) {
        let captured = Typedef.rep(amount);
        return uint256(applyWord(lam (v: word) -> word {
                    let result: word;
                    assembly {
                        result := add(v, captured)
                    }
                    return result;
                }, Typedef.rep(x)));
    }
}
