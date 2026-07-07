function sloadWord() -> word {
  let v : word;
  assembly {
    v := sload(0)
  }
  return v;
}

function leak(comptime x: word) -> comptime word {
  return sloadWord();
}

contract C {
  public function main() -> word {
    return leak(1);
  }
}
