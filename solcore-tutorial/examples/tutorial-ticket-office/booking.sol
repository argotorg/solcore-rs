import * from std;
import * from std.Generic;
import * from std.StorageGeneric;

export { Booking(*), seatsOf, costOf };

// An unset mapping entry is Empty, the first variant.
enum Booking {
    Empty,
    Active(uint256, uint256)
}

function seatsOf(booking: Booking) returns (uint256) {
    match (booking) {
        case Booking.Active(quantity, _) {
            return quantity;
        }
        case Booking.Empty {
            return uint256(0);
        }
    }
}

function costOf(booking: Booking) returns (uint256) {
    match (booking) {
        case Booking.Active(_, quotedCost) {
            return quotedCost;
        }
        case Booking.Empty {
            return uint256(0);
        }
    }
}
