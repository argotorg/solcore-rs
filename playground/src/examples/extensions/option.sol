// Candidate standard-library inventory: this module should disappear once
// std provides Option.
import * from std;
import * from std.Generic;
import * from std.StorageGeneric;

export { Option(*), checkedDiv, unwrapOr, contains };

enum Option<a> {
    None,
    Some(a)
}

// Division by zero yields Option.None instead of reverting.
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

function contains<a>(option: Option<a>, value: a) returns (bool) where a: Eq {
    match (option) {
        case Option.Some(inner) {
            return inner == value;
        }
        default {
            return false;
        }
    }
}
