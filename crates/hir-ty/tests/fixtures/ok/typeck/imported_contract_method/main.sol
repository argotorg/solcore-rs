import * from std;
import * from std.dispatch;
import { setValue } from lib;

// Issue #48: the contract method shadows the imported function. The recursive
// unqualified call is accepted, matching upstream a7b7afb0 (#599).
contract C {
    function setValue(x: uint256) public {
        setValue(x);
    }
}
