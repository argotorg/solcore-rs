
forall a . class  a : Neg {
   function neg(x:a) -> a;
}

data B = F | T;


instance B : Neg {
  function neg (x : B) -> B {
    match x {
    | B.F => return B.T;
    | B.T => return B.F;
    }
  }
}


contract NegBool {

  function fromB(b : B) -> word {
    match b  {
    | B.F => return 0;
    | B.T => return 1;
    }
  }

  // #[() -> 1]
  public function run() -> uint256 { return uint256(fromB(Neg.neg(B.F))); }
}
import std.{*};
import std.dispatch.{*};
