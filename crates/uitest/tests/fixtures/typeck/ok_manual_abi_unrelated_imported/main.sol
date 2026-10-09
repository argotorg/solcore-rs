import * from std;
import * from std.dispatch;
import {unrelated} from lib;

contract C {
  constructor(value: uint256) {}

  function hello() public returns (uint256) { return uint256(7); }

  fallback() { unrelated(); }
}
