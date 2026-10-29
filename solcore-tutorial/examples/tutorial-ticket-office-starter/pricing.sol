import * from std;

export { Standard(*), Premium(*), Priced, quote };

enum Standard {
    Standard
}

enum Premium {
    Premium
}

trait Priced<a> {
    function unitPrice(ticket: a) returns (uint256);
}

impl Priced<Standard> {
    function unitPrice(ticket: Standard) returns (uint256) {
        // TODO 1a: return the standard unit price, 10.
        return uint256(0);
    }
}

impl Priced<Premium> {
    function unitPrice(ticket: Premium) returns (uint256) {
        // TODO 1b: return the premium unit price, 15.
        return uint256(0);
    }
}

// The caller bounds quantity before computing the total.
function quote<a>(ticket: a, quantity: uint256) returns (uint256) where a: Priced {
    // TODO 1c: multiply the trait-provided unit price by quantity.
    return uint256(0);
}
