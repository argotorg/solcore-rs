forall t . class t:Add {
  function add(l: t, r: t) -> t;
}

data Choice = Choice(word);

instance Choice:Add {
  function add(l: Choice, r: Choice) -> Choice {
    return r;
  }
}

function choose_right(x: Choice, y: Choice) -> Choice {
  let result: Choice = x;
  result += y;
  return result;
}
