pragma solcore;

import * from std;
type Units = calldata<array<()>>;
contract C {
    function bad(values: Units) public returns (Units) {
        return values;
    }
    function nested(values: calldata<array<Units>>) public returns (calldata<array<Units>>) {
        return values;
    }
}
