import std.{*};
import std.dispatch.{*};

forall a b . function nestedSnd(p: (a, b)) -> b {
  match p {
  | (_, tail) => return tail;
  }
}

contract NestedPairTail {
  x: word;

  // #[() -> 42]
  public function run() -> uint256 {
    x = 42;
    let tail = nestedSnd((x, (x, x)));
    match tail {
    | (head, _) => return uint256(head);
    }
  }
}
