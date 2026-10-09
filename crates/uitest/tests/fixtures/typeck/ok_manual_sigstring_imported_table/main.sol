import * from std;
import * from std.dispatch;
import {serveTable} from table;

contract C {
  function hello() public returns (uint256) {
    return uint256(7);
  }

  fallback() {
    serveTable();
    revertLit("unknown selector");
  }
}
