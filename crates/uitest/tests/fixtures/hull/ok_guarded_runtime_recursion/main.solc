import std.{*};

function countdown(n: word) -> word {
  if (n == 0) {
    return 0;
  } else {
    return countdown(n - 1);
  }
}

contract Counter {
  public function main() -> word {
    return countdown(3);
  }
}
