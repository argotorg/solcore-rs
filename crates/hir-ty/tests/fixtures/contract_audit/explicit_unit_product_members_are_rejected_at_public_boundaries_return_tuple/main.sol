pragma solcore;

import * from std;
type UnitAlias = ();
contract C {
    function bad() public returns ((uint256, UnitAlias)) {
        return (uint256(0), ());
    }
}
