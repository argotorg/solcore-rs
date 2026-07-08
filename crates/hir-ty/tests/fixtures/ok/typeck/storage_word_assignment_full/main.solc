data storage(t) = storage(word);

forall a b.
class a:CanStore(b) {
  function store(r:a, v:b) -> ();
  function load(r:a) -> b;
}

instance storage(word):CanStore(word) {
  function store(dst: storage(word), src: word) -> () {
    return ();
  }

  function load(src: storage(word)) -> word {
    return 0;
  }
}

contract StorageWordAssign {
  x: word;

  function setx() -> () {
    x = 8;
  }

  public function main() -> word {
    setx();
    return x;
  }
}
