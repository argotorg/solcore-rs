pragma solcore;

import * from std;
type UnitAlias = ();
contract C {
    function bad(x: calldata<array<(uint256, (UnitAlias, uint256))>>) public {}
}
