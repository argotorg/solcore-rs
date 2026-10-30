pragma solcore;

import * from std;
import * from std.dispatch;

// Evaluated while compiling: the deployed code uses the result instead of
// performing this addition when answer() is called.
function double(comptime value: uint256) returns (comptime<uint256>) {
    return value + value;
}

contract Answer {
    // #[() -> 42]
    function answer() public returns (uint256) {
        let result: comptime<uint256> = double(uint256(21));
        return result;
    }
}
