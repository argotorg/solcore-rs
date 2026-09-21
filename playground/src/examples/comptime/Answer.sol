import * from std;
import * from std.dispatch;

// Evaluated during specialization: the deployed code contains only the
// result, not the computation.
function double(comptime value: uint256) returns (comptime<uint256>) {
    return value + value;
}

contract Answer {
    function answer() public returns (uint256) {
        let result: comptime<uint256> = double(uint256(21));
        return result;
    }
}
