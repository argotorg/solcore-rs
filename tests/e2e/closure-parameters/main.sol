import * from std;
import * from std.dispatch;

function makeAdder(value: uint256) returns (function(uint256) returns (uint256)) {
  return lam (other: uint256) -> uint256 { return value + other; };
}

function twice(f: function(uint256) returns (uint256), x: uint256) returns (uint256) {
  return f(f(x));
}

function forward(f: function(uint256) returns (uint256), x: uint256) returns (uint256) {
  return twice(f, x);
}

function repeat(f: function(uint256) returns (uint256), x: uint256, n: uint256) returns (uint256) {
  if (n == uint256(0)) { return x; }
  return repeat(f, f(x), n - uint256(1));
}

function increment(x: uint256) returns (uint256) { return x + uint256(1); }

function run(f: function() returns (uint256)) returns (uint256) { return f(); }

function apply2(f: function(uint256, uint256) returns (uint256), x: uint256, y: uint256) returns (uint256) {
  return f(x, y);
}

function applyWord(f: function(word) returns (word), x: word) returns (word) { return f(x); }

contract ClosureParameters {
  count: uint256;
  locked: bool;

  // #[(0) -> 42]
  // #[(8) -> 50]
  function shifted(x: uint256) public returns (uint256) {
    let f = makeAdder(uint256(10));
    let base = f(uint256(32));
    return base + x;
  }

  // #[() -> 42]
  function fixed() public returns (uint256) {
    return twice(lam (v: uint256) -> uint256 { return v + uint256(10); }, uint256(22));
  }

  // #[(0) -> 20]
  // #[(22) -> 42]
  function addTwenty(x: uint256) public returns (uint256) {
    return twice(lam (v: uint256) -> uint256 { return v + uint256(10); }, x);
  }

  // #[(22) -> 42]
  function factory(x: uint256) public returns (uint256) {
    return twice(makeAdder(uint256(10)), x);
  }

  // #[(40) -> 42]
  function shadowed(x: uint256) public returns (uint256) {
    let v = uint256(99);
    return twice(lam (v: uint256) -> uint256 { return v + uint256(1); }, x);
  }

  // #[(10, 22) -> 42]
  // #[(7, 3) -> 17]
  function captured(amount: uint256, x: uint256) public returns (uint256) {
    return forward(lam (v: uint256) -> uint256 { return v + amount; }, x);
  }

  // #[(10, 22) -> 42]
  function capturedFieldName(count: uint256, x: uint256) public returns (uint256) {
    return twice(lam (v: uint256) -> uint256 { return v + count; }, x);
  }

  // #[(21) -> 42]
  function capturedBeforeAssignment(x: uint256) public returns (uint256) {
    let f: function(uint256) returns (uint256) = lam (v: uint256) -> uint256 { return v + x; };
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
    return repeat(lam (v: uint256) -> uint256 { return v + amount; }, x, n);
  }

  function withLock(f: function(uint256) returns (uint256), x: uint256) returns (uint256) {
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
    let result = twice(lam (v: uint256) -> uint256 { return uint256(11); }, next());
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
    return run(lam () -> uint256 { return x + uint256(2); });
  }

  // #[(10, 20, 12) -> 42]
  function multipleArgs(amount: uint256, x: uint256, y: uint256) public returns (uint256) {
    return apply2(lam (a: uint256, b: uint256) -> uint256 { return a + b + amount; }, x, y);
  }

  // #[(40, 2) -> 42]
  function assemblyCapture(x: uint256, amount: uint256) public returns (uint256) {
    let captured = Typedef.rep(amount);
    return uint256(applyWord(lam (v: word) -> word {
      let result: word;
      assembly { result := add(v, captured) }
      return result;
    }, Typedef.rep(x)));
  }
}
