import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

enum Leaf {
    Leaf(uint256)
}
enum Choice {
    First(Leaf),
    Last(uint256)
}

impl SigString<Leaf> {
    function sigStr(p: Proxy<Leaf>) returns (string) {
        return "changed";
    }
}

contract C {
    // The field signature changes First's wire tag even with no input selector.
    function value() public returns (Choice) {
        return Choice.First(Leaf(uint256(7)));
    }
}
