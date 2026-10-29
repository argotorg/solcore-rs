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

    // These two smoke tests cover the supplied scaffold, not the finished exercise.
    // #[() -> 12]
    function capacity() public returns (uint256) {
        let total: comptime<uint256> = venueCapacity(uint256(3), uint256(4));
        return total;
    }

    function quoteStandard(quantity: uint256) public returns (uint256) {
        require(quantity > uint256(0), "choose at least one seat");
        require(quantity <= capacity(), "quantity exceeds capacity");
        return quote(Standard, quantity);
    }

    function quotePremium(quantity: uint256) public returns (uint256) {
        require(quantity > uint256(0), "choose at least one seat");
        require(quantity <= capacity(), "quantity exceeds capacity");
        return quote(Premium, quantity);
    }

    function validate(quantity: uint256) {
        require(isOpen(), "sales closed");
        // TODO 3a: reject zero seats, an existing booking, and insufficient capacity.
        revertWithError("finish reservation validation");
    }

    function record(quantity: uint256, quotedCost: uint256) returns (uint256) {
        // TODO 3b: store Active(quantity, quotedCost), update reserved, and return the cost.
        revertWithError("finish recording a booking");
        return uint256(0);
    }

    function reserveStandard(quantity: uint256) public returns (uint256) {
        validate(quantity);
        return record(quantity, quoteStandard(quantity));
    }

    function mySeats() public returns (uint256) {
        return seatsOf(bookings[sender()]);
    }

    function myQuote() public returns (uint256) {
        return costOf(bookings[sender()]);
    }

    function cancel() public returns (uint256) {
        require(isOpen(), "sales closed");
        // TODO 4: match this caller's booking, return its seats, clear it, and return its cost.
        // Revert when the booking is Empty.
        revertWithError("finish cancellation");
        return uint256(0);
    }

    function reservePremium(quantity: uint256) public returns (uint256) {
        validate(quantity);
        return record(quantity, quotePremium(quantity));
    }

    function remaining() public returns (uint256) {
        return capacity() - reserved;
    }

    function close() public {
        require(sender() == organizer, "organizer only");
        require(isOpen(), "already closed");
        sale = Sale.Closed;
    }

    // #[() -> true]
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
