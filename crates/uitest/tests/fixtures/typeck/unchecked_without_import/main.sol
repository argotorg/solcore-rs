import * from std;

function f(x: uint256, y: uint256) returns (uint256) {
    unchecked {
        return x + y;
    }
}
