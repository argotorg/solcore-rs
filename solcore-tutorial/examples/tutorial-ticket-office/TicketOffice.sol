import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;
import { Standard, Premium, quote } from pricing;
import { Booking, seatsOf, costOf } from booking;
import { sender } from context;

enum Sale {
    Open,
    Closed
}

function venueCapacity(comptime rows: uint256, comptime seatsPerRow: uint256)
returns (comptime<uint256>) {
    return rows * seatsPerRow;
}

// A reservation ledger: quoted costs are numbers, not payments.
contract TicketOffice {
    organizer: address;
    sale: Sale;
    reserved: uint256;
    bookings: mapping(address => Booking);

    constructor() {
        organizer = sender();
        sale = Sale.Open;
    }

    // #[() -> 12]
    function capacity() public returns (uint256) {
        let total: comptime<uint256> = venueCapacity(uint256(3), uint256(4));
        return total;
    }

    // #[(2) -> 20]
    function quoteStandard(quantity: uint256) public returns (uint256) {
        require(quantity > uint256(0), "choose at least one seat");
        require(quantity <= capacity(), "quantity exceeds capacity");
        return quote(Standard, quantity);
    }

    // #[(2) -> 30]
    function quotePremium(quantity: uint256) public returns (uint256) {
        require(quantity > uint256(0), "choose at least one seat");
        require(quantity <= capacity(), "quantity exceeds capacity");
        return quote(Premium, quantity);
    }

    function validate(quantity: uint256) {
        require(isOpen(), "sales closed");
        require(quantity > uint256(0), "choose at least one seat");
        require(seatsOf(bookings[sender()]) == uint256(0), "already reserved");
        require(quantity <= remaining(), "not enough seats");
    }

    function record(quantity: uint256, quotedCost: uint256) returns (uint256) {
        bookings[sender()] = Booking.Active(quantity, quotedCost);
        reserved = reserved + quantity;
        return quotedCost;
    }

    // Expectations discard their changes; only the send keeps the booking.
    // #[(0) -> revert]
    // #[(13) -> revert]
    // #[(2) -> 20]
    // #[send(3)]
    // #[(1) -> revert]
    function reserveStandard(quantity: uint256) public returns (uint256) {
        validate(quantity);
        return record(quantity, quoteStandard(quantity));
    }

    // #[() -> 3]
    function mySeats() public returns (uint256) {
        return seatsOf(bookings[sender()]);
    }

    // #[() -> 30]
    function myQuote() public returns (uint256) {
        return costOf(bookings[sender()]);
    }

    // #[() -> 30]
    // #[send()]
    // #[() -> revert]
    function cancel() public returns (uint256) {
        require(isOpen(), "sales closed");
        match (bookings[sender()]) {
            case Booking.Active(quantity, quotedCost) {
                reserved = reserved - quantity;
                bookings[sender()] = Booking.Empty;
                return quotedCost;
            }
            case Booking.Empty {
                revertWithError("no booking");
                return uint256(0);
            }
        }
    }

    // #[send(2)]
    function reservePremium(quantity: uint256) public returns (uint256) {
        validate(quantity);
        return record(quantity, quotePremium(quantity));
    }

    // #[() -> 10]
    function remaining() public returns (uint256) {
        return capacity() - reserved;
    }

    // #[send()]
    // #[() -> revert]
    function close() public {
        require(sender() == organizer, "organizer only");
        require(isOpen(), "already closed");
        sale = Sale.Closed;
    }

    // #[() -> false]
    function isOpen() public returns (bool) {
        match (sale) {
            case Sale.Open {
                return true;
            }
            case Sale.Closed {
                return false;
            }
        }
    }
}
