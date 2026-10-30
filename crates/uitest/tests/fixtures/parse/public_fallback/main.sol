pragma solcore;

contract Bad {
  fallback() public {}

  function after() {}
}
