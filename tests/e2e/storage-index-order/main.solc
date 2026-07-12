import std.{*};
import std.dispatch.{*};

contract StorageIndexOrder {
  counter: word;
  m: mapping(word, word);

  function next() -> word {
    let cur: word = counter;
    let res: word;
    assembly {
      res := add(cur, 1)
    }
    counter = res;
    return res;
  }

  // #[() -> 2]
  public function run() -> uint256 {
    counter = 0;
    m[1] = 0;
    m[2] = 0;
    m[next()] = next();

    let one: word = m[1];
    let two: word = m[2];
    let packed: word;
    assembly {
      packed := add(one, mul(two, 10))
    }
    return uint256(packed);
  }

  function get(k: word) -> word {
    return m[k];
  }
}
