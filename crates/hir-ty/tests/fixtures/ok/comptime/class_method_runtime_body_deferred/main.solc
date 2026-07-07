data Box = Box(word);

forall a. class a : Scale {
  function scale(comptime factor : word, comptime x : a) -> comptime a;
}

instance word : Scale {
  function scale(comptime factor : word, comptime x : word) -> comptime word {
    return x;
  }
}

instance Box : Scale {
  function scale(comptime factor : word, comptime x : Box) -> comptime Box {
    let y : word;
    assembly {
      y := sload(0)
    }
    return Box(y);
  }
}

contract C {
  function main() -> word {
    let a : comptime word = Scale.scale(1, 2);
    return a;
  }
}
