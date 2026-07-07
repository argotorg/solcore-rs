contract C {
  public function main() -> word {
    let x : word;
    assembly {
      function dbl(a) -> r {
        r := add(a, a)
      }
      x := dbl(1, 2)
    }
    return x;
  }
}
