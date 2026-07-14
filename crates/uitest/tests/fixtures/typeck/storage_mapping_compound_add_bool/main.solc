data mapping(key, value) = mapping(word);
data uint256 = uint256(word);

forall t . class t:Add {
  function add(l: t, r: t) -> t;
}
forall t . class t:Sub {
  function sub(l: t, r: t) -> t;
}
instance word:Add {
  function add(l: word, r: word) -> word { return l; }
}
instance word:Sub {
  function sub(l: word, r: word) -> word { return l; }
}
instance uint256:Add {
  function add(l: uint256, r: uint256) -> uint256 { return l; }
}

contract C {
  m: mapping(word, bool);
  function f(k: word) -> () { m[k] += true; }
  function main() -> () { return (); }
}
