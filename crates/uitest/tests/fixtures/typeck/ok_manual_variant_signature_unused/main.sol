pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum Choice {
    First(uint256),
    Last(uint256)
}

// The wire tags use each constructor's fields, never Choice's own signature.
impl SigString<Choice> {
    function sigStr(p: Proxy<Choice>) returns (string) {
        return "unused";
    }
}

contract C {
    function value() public returns (Choice) {
        return Choice.First(uint256(7));
    }
}
