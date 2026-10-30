pragma solcore;

import * from std;
import * from std.dispatch;
import { serveTable } from table;

// #[scenario("C")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x19ff1d21, label = "hello(): generated dispatch") -> success(0x0000000000000000000000000000000000000000000000000000000000000007)]
// #[raw(0x85bb7d69, label = "answer(): manual dispatch reached through the fallback") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0xdeadc0de, label = "unknown selector: manual dispatch misses and fallback reverts") -> revert(0x756e6b6e6f776e2073656c6563746f72)]
contract C {
    function hello() public returns (uint256) {
        return uint256(7);
    }

    fallback() {
        serveTable();
        revertLit("unknown selector");
    }
}
