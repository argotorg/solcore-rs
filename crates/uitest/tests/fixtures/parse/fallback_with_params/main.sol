pragma solcore;

contract Bad {
  fallback(x: word) {}

  function after() {}
}
