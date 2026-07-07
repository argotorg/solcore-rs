import lib.{wrapper, boxed};

// Same-name constructors from a selective import stay legal unqualified in
// both pattern and expression position.
function unwrap(u: wrapper) -> word {
  match u {
  | wrapper(w) => return w;
  }
}

function rebox(b: boxed) -> boxed {
  match b {
  | boxed(w) => return boxed(w);
  }
}

function main() -> word {
  return unwrap(wrapper(3));
}
