pragma solcore;

contract Bad {
  fallback() returns (word) {}

  function after() {}
}
