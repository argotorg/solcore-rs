import std.{*};

data Box = Box(word);

instance Box : StorageType {
  function load(ptr : word) -> Box {
    return Box(StorageType.load(ptr):word);
  }

  function store(ptr : word, value : Box) -> () {
    match value {
      | Box(inner) => StorageType.store(ptr, inner);
    }
  }
}

instance storage(Box) : CanStore(Box) {
  function load(ptr : storage(Box)) -> Box {
    return StorageType.load(Typedef.rep(ptr)):Box;
  }

  function store(ptr : storage(Box), value : Box) -> () {
    StorageType.store(Typedef.rep(ptr), value);
  }
}

contract DataTypeStorageFull {
  box : Box;

  public function main() -> word {
    match box {
      | Box(inner) => return inner;
    }
  }
}
