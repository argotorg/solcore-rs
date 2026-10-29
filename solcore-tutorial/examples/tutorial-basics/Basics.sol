import * from std;
import * from std.dispatch;

function double(value: uint256) returns (uint256) {
    return value + value;
}

contract Basics {
    // A local binding can be reassigned during the call.
    // #[(21, true) -> 42]
    // #[(21, false) -> 21]
    // #[(0, true) -> 0]
    function adjust(value: uint256, bonus: bool) public returns (uint256) {
        let result: uint256 = value;
        if (bonus) {
            result = double(result);
        }
        return result;
    }

    // Add the integers from zero up to, but not including, limit.
    // Keep limit small when experimenting: each iteration costs gas.
    // #[(0) -> 0]
    // #[(5) -> 10]
    function sumBelow(limit: uint256) public returns (uint256) {
        let total: uint256 = uint256(0);
        for (let i: uint256 = uint256(0); i < limit; i = i + uint256(1)) {
            total = total + i;
        }
        return total;
    }
}
