trait Add<t> {
  function add(l: t, r: t) returns (t) ;
}

enum Choice { Choice(word) }

impl Add<Choice> {
  function add(l: Choice, r: Choice) returns (Choice) {
    return r;
  }
}

function choose_right(x: Choice, y: Choice) returns (Choice) {
  let result: Choice = x;
  result += y;
  return result;
}
