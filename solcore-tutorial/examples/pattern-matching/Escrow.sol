import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

// An escrow whose lifecycle is an enum stored in a state variable.
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
                revertWithError("already funded");
            }
        }
    }

    // Record the release so that it cannot be repeated.
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
                revertWithError("nothing to release");
                return 0;
            }
        }
    }

    // 0 = awaiting payment, 1 = funded, 2 = released
    // #[() -> 2]
    function status() public returns (uint256) {
        match (phase) {
            case Phase.AwaitingPayment {
                return 0;
            }
            case Phase.Funded(_) {
                return 1;
            }
            case Phase.Released(_) {
                return 2;
            }
        }
    }
}
