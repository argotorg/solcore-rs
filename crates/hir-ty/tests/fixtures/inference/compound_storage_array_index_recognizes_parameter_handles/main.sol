pragma solcore;

import * from std;

function bump(values: storage<array<uint256>>, index: uint256, delta: uint256) {
    values[index] += delta;
    return ();
}
