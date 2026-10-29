pragma solcore;

// return-word: 42
import * from std;
import * from std.dispatch;
contract Answer {
    function a_main_helper() returns (word) {
        let value: word;
        assembly {
            value := calldatasize()
        }
        return value;
    }
    function main() public returns (uint256) {
        let value = a_main_helper();
        assembly {
            value := add(value, 42)
        }
        return uint256(value);
    }
}
