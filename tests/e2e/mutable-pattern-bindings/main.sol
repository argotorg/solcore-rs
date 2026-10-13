import * from std;
import * from std.dispatch;

function knownIfBinding() returns (word) {
  let x: word = 1;
  if (true) { let x: word = 2; }
  return x;
}

function comptimePatternWrite() returns (comptime<word>) {
  let values: (word, word) = (40, 2);
  match (values) { case (a, b) { a = b; return a; } }
}

function comptimeAssemblyPatternWrite() returns (comptime<word>) {
  let values: (word, word) = (40, 2);
  match (values) {
    case (a, b) { assembly { a := add(a, b) } return a; }
  }
}

function addPatternPair(values: (word, word)) returns (word) {
  match (values) {
    case (a, b) {
      let f: function(word) returns (word) = lam (x: word) -> word {
        let result: word;
        assembly { result := add(x, b) }
        return result;
      };
      return f(a);
    }
  }
}

contract MutablePatternBindings {
  count: uint256;

  function changeCount() returns (()) { count = uint256(42); return; }
  // #[() -> 42]
  function assemblyWrite() public returns (uint256) {
    let result: word;
    let values: (word, word) = (0, 0);
    match (values) {
      case (v0, v1) {
        assembly { v1 := 42 }
        let after: word = v1;
        assembly { result := after }
      }
    }
    return uint256(result);
  }

  // #[() -> 42]
  function ordinaryWrite() public returns (uint256) {
    let values: (word, word) = (0, 0);
    match (values) {
      case (x, y) { y = 42; return uint256(y); }
    }
  }

  // #[() -> 83]
  function outerShadow() public returns (uint256) {
    let y: word = 41;
    let result: word;
    let values: (word, word) = (0, 0);
    match (values) {
      case (x, y) { assembly { y := 42 result := y } }
    }
    assembly { result := add(result, y) }
    return uint256(result);
  }

  // #[() -> 7]
  function assemblyShadow() public returns (uint256) {
    let result: word;
    let values: (word, word) = (0, 7);
    match (values) {
      case (x, y) {
        assembly { { let y := 99 result := y } result := y }
      }
    }
    return uint256(result);
  }
  // #[() -> 42]
  function firstArm() public returns (uint256) {
    let values: (word, word) = (0, 0);
    match (values) {
      case (x, y) { assembly { y := 42 } return uint256(y); }
      default { return uint256(99); }
    }
  }

  // #[() -> 48]
  function innerShadow() public returns (uint256) {
    let y: word = 41;
    let result: word;
    let values: (word, word) = (0, 0);
    match (values) {
      case (x, y) {
        let y: word = 7;
        assembly { result := y }
      }
    }
    assembly { result := add(result, y) }
    return uint256(result);
  }

  // #[() -> 41]
  function readOnlyShadow() public returns (uint256) {
    let y: word = 41;
    let result: word;
    let values: (word, word) = (0, 0);
    match (values) { case (x, y) { result = y; } }
    assembly { result := add(result, y) }
    return uint256(result);
  }

  // #[() -> 42]
  function comptimeAfterMatch() public returns (uint256) {
    let result: word = 0;
    let input: word = 0;
    match (input) { case 0 { result = 42; } default { result = 99; } }
    let answer: comptime<word> = result;
    return uint256(answer);
  }

  // #[() -> 42]
  function comptimeAfterBlock() public returns (uint256) {
    let result: word = 0;
    { result = 42; }
    let answer: comptime<word> = result;
    return uint256(answer);
  }

  // #[() -> 42]
  function nestedScopeUpdate() public returns (uint256) {
    let result: word = 1;
    let values: (word, word) = (0, 0);
    match (values) {
      case (x, y) {
        result = 42;
        { let result: word = 99; }
      }
    }
    let answer: comptime<word> = result;
    return uint256(answer);
  }

  // #[() -> 42]
  function writeBeforeShadow() public returns (uint256) {
    let result: word = 1;
    let input: word = 0;
    match (input) {
      case 0 { result = 42; let result: word = 99; }
      default { result = 99; }
    }
    let answer: comptime<word> = result;
    return uint256(answer);
  }

  // #[() -> 42]
  function storageShadow() public returns (uint256) {
    count = uint256(1);
    let values: (word, word) = (0, 0);
    match (values) { case (x, count) { changeCount(); } }
    return count;
  }

  // #[() -> 42]
  function blockStorageShadow() public returns (uint256) {
    count = uint256(1);
    { let count: word = 99; changeCount(); }
    return count;
  }

  // #[() -> 7]
  function localFieldNameShadow() public returns (uint256) {
    let count: word = 7;
    { let count: word = 99; changeCount(); }
    return uint256(count);
  }

  // #[() -> 2]
  function knownIfRebinding() public returns (uint256) {
    let x: word = 1;
    if (true) { let x: word = 2; }
    return uint256(x);
  }

  // #[() -> 42]
  function knownIfDeclaration() public returns (uint256) {
    if (true) { let x: word = 42; }
    let answer: comptime<word> = x;
    return uint256(answer);
  }

  // #[() -> 2]
  function pureIfRebinding() public returns (uint256) {
    return uint256(knownIfBinding());
  }

  // #[(1) -> 2]
  // #[(0) -> 3]
  function runtimeIfRebinding(flag: uint256) public returns (uint256) {
    let x: word = 1;
    if (flag == uint256(1)) { let x: word = 2; }
    else { x = 3; }
    return uint256(x);
  }

  // #[() -> 2]
  function comptimeWrite() public returns (uint256) {
    return uint256(comptimePatternWrite());
  }

  // #[() -> 42]
  function comptimeAssemblyWrite() public returns (uint256) {
    return uint256(comptimeAssemblyPatternWrite());
  }

  // #[(40, 2) -> 42]
  // #[(3, 7) -> 10]
  function patternAssemblyCapture(x: uint256, y: uint256) public returns (uint256) {
    return uint256(addPatternPair((Typedef.rep(x), Typedef.rep(y))));
  }

  // #[() -> 83]
  function patternCaptureWrite() public returns (uint256) {
    let values: (word, word) = (0, 41);
    match (values) {
      case (a, b) {
        let f: function() returns (word) = lam () -> word {
          assembly { b := add(b, 1) sstore(100, b) }
          return b;
        };
        let result: word = f();
        assembly { result := add(result, b) }
        return uint256(result);
      }
    }
  }

  // #[() -> 1]
  function nestedIfScope() public returns (uint256) {
    let x: word = 1;
    { if (true) { let x: word = 2; } }
    let answer: comptime<word> = x;
    return uint256(answer);
  }

  // #[() -> 42]
  function nestedIfWriteBeforeShadow() public returns (uint256) {
    let x: word = 1;
    { if (true) { x = 42; let x: word = 99; } }
    let answer: comptime<word> = x;
    return uint256(answer);
  }

  // #[(1) -> 1]
  // #[(0) -> 1]
  function runtimeNestedIfScope(flag: uint256) public returns (uint256) {
    let x: word = 1;
    { if (flag == uint256(1)) { let x: word = 2; } }
    let answer: comptime<word> = x;
    return uint256(answer);
  }

  // #[(1) -> 42]
  // #[(0) -> 1]
  function runtimeNestedIfWriteBeforeShadow(flag: uint256) public returns (uint256) {
    let x: word = 1;
    { if (flag == uint256(1)) { x = 42; let x: word = 99; } }
    return uint256(x);
  }

}
