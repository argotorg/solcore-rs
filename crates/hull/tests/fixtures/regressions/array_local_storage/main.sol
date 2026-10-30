pragma solcore;

//~ hull: contains "LValueIdxAccess_lookup"
//~ hull: contains "std_lidx"
//~ hull: contains "std_out_of_bounds"

import * from std;

function main() returns (uint256) {
    let xs: storage<array<uint256>> = storage(0x100);
    return xs[uint256(0)];
}
