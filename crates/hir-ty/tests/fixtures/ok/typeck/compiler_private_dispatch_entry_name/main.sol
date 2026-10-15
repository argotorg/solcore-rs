import * from std;
import * from std.dispatch;

function main(x: uint256) returns (uint256) {
    return x;
}

contract C {
    function call_top() returns (uint256) {
        return main(uint256(1));
    }
    function ping(x: uint256) public returns (uint256) {
        return x;
    }
}
