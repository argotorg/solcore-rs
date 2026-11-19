pragma solcore;

// Each iteration wraps the previous closure in a new one. The trip count is
// known at compile time, but the loop is not unrolled.
import * from std;
import * from std.dispatch;

contract C {
    function run(input: uint256) public returns (uint256) {
        let f: function (uint256) returns (uint256) = lam (v: uint256) -> uint256 {
            return v;
        };
        for (let i: uint256 = 0; i < 3; i = i + 1) {
            f = lam (v: uint256) -> uint256 {
                return f(v) + 1;
            };
        }
        return f(input);
    }
}
