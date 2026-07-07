contract YulMultiRetBad {
  public function main() -> word {
    let x : word;
    let y : word;
    let z : word;
    assembly {
      function pair() -> a, b {
        a := 1
        b := 2
      }
      x, y, z := pair()
    }
    return x;
  }
}
