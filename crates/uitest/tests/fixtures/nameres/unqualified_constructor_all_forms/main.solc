data Option = None | Some(word);
data flag = off | on;

function exprCall(x: word) -> Option { return Some(x); }
function exprBare(f: flag) -> flag { return on; }

function patLower(f: flag) -> word {
  match f {
  | off => return 0;
  | on => return 1;
  }
}

function patUpper(o: Option) -> word {
  match o {
  | None => return 0;
  | _ => return 1;
  }
}
