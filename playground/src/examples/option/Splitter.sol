import * from std;
import * from std.dispatch;
import {Option, checkedDiv, unwrapOr} from option;

contract Splitter {
    // Each recipient's equal share of the pot; zero recipients yields
    // zero instead of reverting on division.
    // #[(10, 3) -> 3]
    // #[(10, 0) -> 0]
    function share(pot: uint256, recipients: uint256) public returns (uint256) {
        return unwrapOr(checkedDiv(pot, recipients), uint256(0));
    }

    // What is left over after handing out equal shares.
    // #[(10, 3) -> 1]
    // #[(10, 0) -> 10]
    function remainder(pot: uint256, recipients: uint256) public returns (uint256) {
        match (checkedDiv(pot, recipients)) {
            case Option.Some(perRecipient) { return pot - perRecipient * recipients; }
            default { return pot; }
        }
    }
}
