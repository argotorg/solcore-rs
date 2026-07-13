data B = A | C;

function choose(x : bool) -> B {
  if (x) {
    return B.A;
  }
  return B.C;
}

function onlyA(b : B) -> word {
  match b {
    | B.A => return 1;
  }
}

contract C {
  public function main() -> word {
    let x: bool;
    assembly { x := calldataload(0) }
    return onlyA(choose(x));
  }
}
