pragma solcore;

import * from std;
type UnitAlias = ();
contract C {
    function bad(x: calldata<array<(UnitAlias, uint256)>>) public {}
}
