pragma solcore;

import * from std;

contract InputOnly {
    function keep(values: calldata<array<uint256>>) public returns (uint256, calldata<array<uint256>>) {
        return (uint256(0), values);
    }
}
