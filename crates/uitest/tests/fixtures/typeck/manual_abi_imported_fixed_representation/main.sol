import * from std;
import * from std.dispatch;
import {Fixed, Decoded} from lib;

contract C {
  constructor(value: Decoded) {}

  function inspect(value: Fixed) public returns (uint256) {
    return uint256(7);
  }
}
