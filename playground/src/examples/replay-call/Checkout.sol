pragma solcore;
import * from std;
import * from std.dispatch;
import { withFee } from fee;

contract Checkout {
    function checkout(amount: uint256) public returns (uint256) {
        let total: uint256 = uint256(withFee(toWord(amount)));
        require(total <= uint256(20), "Over limit");
        return total;
    }
}
