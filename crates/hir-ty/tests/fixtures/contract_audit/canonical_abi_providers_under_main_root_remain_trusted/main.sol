pragma solcore;

import * from std;
import * from dispatch;
import * from std.Generic;
import * from ABIGeneric;
enum Choice {
    First(uint256),
    Second(bool)
}
contract C {
    constructor(value: Choice) {}
    function echo(value: Choice) public returns (Choice) {
        return value;
    }
    function take(values: calldata<array<Choice>>) public {}
}
