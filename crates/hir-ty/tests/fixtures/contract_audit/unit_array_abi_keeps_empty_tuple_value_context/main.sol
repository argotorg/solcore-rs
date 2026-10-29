pragma solcore;

import * from std;
type UnitAlias = ();
type Units = calldata<array<UnitAlias>>;
contract UnitArrays {
    constructor(seed: uint256) {}
    function units(values: Units) public returns (uint256) {
        return uint256(0);
    }
    function nested(values: calldata<array<Units>>) public returns (uint256) {
        return uint256(0);
    }
    function empty() public returns (UnitAlias) {
        return;
    }
    function wrapped(values: calldata<array<(Units, uint256)>>) public returns (uint256) {
        return uint256(0);
    }
}
