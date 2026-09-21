import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

// One generic maximum for every ordered type, user-defined included.
// Candidate standard-library inventory: a generic max belongs in std
// eventually.
function max<a>(x: a, y: a) returns (a) where a: Ord {
    if (x > y) { return x; }
    return y;
}

enum Version {
    Version(uint256, uint256)
}

function major(v: Version) returns (uint256) {
    match (v) {
        case Version(value, _) { return value; }
    }
}

function minor(v: Version) returns (uint256) {
    match (v) {
        case Version(_, value) { return value; }
    }
}

impl Eq<Version> {
    function eq(a: Version, b: Version) returns (bool) {
        return major(a) == major(b) && minor(a) == minor(b);
    }
}

// Ord requires Eq: the compiler checks the trait hierarchy.
impl Ord<Version> {
    function gt(a: Version, b: Version) returns (bool) {
        if (major(a) == major(b)) { return minor(a) > minor(b); }
        return major(a) > major(b);
    }
}

contract Registry {
    newest : Version;

    constructor() {
        newest = Version(uint256(0), uint256(0));
    }

    function publish(maj: uint256, min: uint256) public {
        newest = max(newest, Version(maj, min));
    }

    function newestMajor() public returns (uint256) {
        return major(newest);
    }

    function newestMinor() public returns (uint256) {
        return minor(newest);
    }
}
