pragma solcore;

import * from std;

contract NestedArrays {
    function first(values: calldata<array<calldata<array<uint256>>>>) public returns (uint256) {
        return uint256(0);
    }
}
