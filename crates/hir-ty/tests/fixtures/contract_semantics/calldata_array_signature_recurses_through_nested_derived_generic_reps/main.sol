pragma solcore;

import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Inner {
    Number(uint256),
    Account(address)
}
enum Outer {
    Outer(Inner, bytes32)
}

contract Nested {
    function inspect(values: calldata<array<Outer>>) public returns (uint256) {
        return uint256(0);
    }
}
