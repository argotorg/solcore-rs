pragma solcore;

import * from std;
type UnitAlias = ();
contract C {
    function bad(x: (UnitAlias, uint256)) public {}
}
