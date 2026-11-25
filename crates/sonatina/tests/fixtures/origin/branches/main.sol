pragma solcore;
import * from std;
import * from std.dispatch;

contract Branches {
    function pick(x: uint256) public returns (uint256) {
        let r: uint256 = x;
        if (x == uint256(1)) {
            r = r + uint256(10);
        } else {
            r = r + uint256(20);
        }
        if (r == uint256(11)) {
            r = r + uint256(1);
        }
        return r;
    }
}
