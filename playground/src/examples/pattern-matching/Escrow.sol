import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

// An escrow whose lifecycle is a sum type stored in a contract field.
enum Phase {
    AwaitingPayment,
    Funded(uint256),
    Released(uint256)
}

contract Escrow {
    phase: Phase;

    constructor() {
        phase = Phase.AwaitingPayment;
    }

    // Fund once; a second deposit is rejected.
    // #[send(42)]
    // #[(7) -> revert]
    function deposit(amount: uint256) public {
        match (phase) {
            case Phase.AwaitingPayment {
                phase = Phase.Funded(amount);
            }
            default {
                require(false, "already funded");
            }
        }
    }

    // Commit the release before checking that it cannot be repeated.
    // #[() -> 42]
    // #[send()]
    // #[() -> revert]
    function release() public returns (uint256) {
        match (phase) {
            case Phase.Funded(amount) {
                phase = Phase.Released(amount);
                return amount;
            }
            default {
                require(false, "nothing to release");
                return uint256(0);
            }
        }
    }

    // 0 = awaiting payment, 1 = funded, 2 = released
    // #[() -> 2]
    function status() public returns (uint256) {
        match (phase) {
            case Phase.AwaitingPayment { return uint256(0); }
            case Phase.Funded(_) { return uint256(1); }
            case Phase.Released(_) { return uint256(2); }
        }
    }
}
