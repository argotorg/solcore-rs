data Inner = A | B;
data Outer = Other | Wrap(Inner);

function pick(x : Outer) -> word {
  match x {
  | Outer.Other => return 0;
  | Outer.Wrap(Inner.A) => return 1;
  }
}
