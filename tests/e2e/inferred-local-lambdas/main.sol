import * from std;
import * from std.dispatch;

type W = word;

function forwardValue<a>(x: a) returns (a) { return x; }

function applyBound<f>(callable: f, x: uint256) returns (uint256)
    where f: invokable<uint256, uint256> {
  assembly { sstore(100, 1) }
  return invokable.invoke(callable, x);
}

function genericIdentity<a>(x: a) returns (a) {
  let f = lam (y: a) -> a { return y; };
  return f(x);
}

contract InferredLocalLambdas {
  // #[() -> 42]
  function constant() public returns (uint256) {
    let f = lam (x: word) -> word {
      let y: word = 42;
      return y;
    };
    return uint256(f(1));
  }

  // #[(40) -> 42]
  // #[(10) -> 12]
  function captured(x: uint256) public returns (uint256) {
    let f = lam (v: uint256) -> uint256 { return v + x; };
    x = uint256(99);
    return f(uint256(2));
  }

  // #[(40) -> 42]
  function annotated(x: uint256) public returns (uint256) {
    let f: function(uint256) returns (uint256) = lam (v: uint256) -> uint256 { return v + x; };
    x = uint256(99);
    return f(uint256(2));
  }

  // #[(40) -> 41]
  function nullary(x: uint256) public returns (uint256) {
    let raw = Typedef.rep(x);
    let f = lam () -> word {
      let result: word;
      assembly { result := add(raw, 1) }
      return result;
    };
    return uint256(f());
  }

  // #[(20, 22) -> 42]
  function pairArgs(x: uint256, y: uint256) public returns (uint256) {
    let f = lam (a: uint256, b: uint256) -> uint256 { return a + b; };
    return f(x, y);
  }

  // #[(42) -> 42]
  function generic(x: uint256) public returns (uint256) {
    return genericIdentity(x);
  }

  // #[(40) -> 42]
  // #[(3) -> 5]
  function genericCallable(x: uint256) public returns (uint256) {
    let f = lam (y: uint256) -> uint256 { return y + x; };
    let g = forwardValue(f);
    return g(uint256(2));
  }

  // #[(40) -> 43]
  // #[(3) -> 6]
  function genericBoundCallable(x: uint256) public returns (uint256) {
    assembly { sstore(100, 0) }
    let f = lam (y: uint256) -> uint256 { return y + x; };
    let result = applyBound(f, uint256(2));
    let effect: word;
    assembly { effect := sload(100) }
    return result + uint256(effect);
  }
  // #[(42) -> 42]
  function aliased(x: uint256) public returns (uint256) {
    let f = lam (y: W) -> W { return y; };
    return uint256(f(Typedef.rep(x)));
  }

  // #[(42) -> 42]
  function nested(x: uint256) public returns (uint256) {
    let f = lam (y: word) -> word {
      let g = lam (z: word) -> word { return z; };
      return g(y);
    };
    return uint256(f(Typedef.rep(x)));
  }

}
