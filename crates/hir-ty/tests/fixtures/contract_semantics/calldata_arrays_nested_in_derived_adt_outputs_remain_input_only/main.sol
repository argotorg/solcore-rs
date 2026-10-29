pragma solcore;

import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Bag {
    Bag(calldata<array<uint256>>)
}
enum Outer {
    Outer(Bag)
}

contract InvalidOutputs {
    function bag(values: calldata<array<uint256>>) public returns (Bag) {
        return Bag(values);
    }
    function outer(values: calldata<array<uint256>>) public returns (Outer) {
        return Outer(Bag(values));
    }
}
