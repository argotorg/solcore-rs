// Candidate standard-library inventory: this module should disappear once
// std provides Option.
import * from std;
import * from std.Generic;
import * from std.StorageGeneric;

export { Option(*), checkedDiv, unwrapOr };

enum Option<a> {
    None,
    Some(a)
}

// A zero divisor explicitly yields Option.None rather than a numeric result.
function checkedDiv(a: uint256, b: uint256) returns (Option<uint256>) {
    if (b == uint256(0)) {
        return Option.None;
    }
    return Option.Some(a / b);
}

function unwrapOr<a>(option: Option<a>, orElse: a) returns (a) {
    match (option) {
        case Option.Some(value) {
            return value;
        }
        default {
            return orElse;
        }
    }
}
