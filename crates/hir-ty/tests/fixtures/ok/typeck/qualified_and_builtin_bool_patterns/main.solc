data flag = off | on;

function pick(f: flag) -> word {
  match f {
  | flag.off => return 0;
  | flag.on => return 1;
  }
}

function flip(b: bool) -> word {
  match b {
  | true => return 1;
  | false => return 0;
  }
}

function main() -> word {
  return primAddWord(pick(flag.on), flip(true));
}
