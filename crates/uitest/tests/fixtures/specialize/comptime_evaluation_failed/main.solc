function sloadWord() -> word {
  let v : word;
  assembly {
    v := sload(0)
  }
  return v;
}

contract C {
  public function main() -> word {
    let y : comptime word = sloadWord();
    return y;
  }
}
