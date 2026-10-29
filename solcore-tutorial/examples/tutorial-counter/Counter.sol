import * from std;
import * from std.dispatch;

contract Counter {
    count: uint256;

    // Sends preserve their changes for later tests in this contract.
    // #[send(2)]
    // #[send(3)]
    function add(amount: uint256) public {
        count = count + amount;
    }

    // #[() -> 5]
    function read() public returns (uint256) {
        return count;
    }
}
