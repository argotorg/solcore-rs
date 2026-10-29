pragma solcore;

import * from std;
import * from std.dispatch;

type Balances = mapping(uint256 => uint256);

contract C {
    balances: Balances;

    function roundtrip(k: uint256, v: uint256) public returns (uint256) {
        balances[k] = v;
        return balances[k];
    }
}
