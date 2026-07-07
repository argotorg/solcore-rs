forall a . function leak() -> a {
  let y : a;
  return y;
}

contract C {
  public function main() -> () {
    let x = leak();
    return ();
  }
}
