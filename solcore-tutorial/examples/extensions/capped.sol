import * from std;
import * from std.Generic;
import * from std.StorageGeneric;

// Capped is exported without its constructor and fields: other modules
// create and read it only through the functions below.
export { Capped, cappedAt, requireWithinCap };

// The maximum total supply.
struct Capped {
    cap: uint256;
}

function cappedAt(cap: uint256) returns (Capped) {
    return Capped(cap);
}

// Reverts if supply is above the cap.
function requireWithinCap(state: Capped, supply: uint256) {
    require(supply <= state.cap, "cap exceeded");
}
