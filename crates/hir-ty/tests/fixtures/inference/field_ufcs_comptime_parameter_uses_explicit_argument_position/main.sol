pragma solcore;

import { array, storage } from std;

trait Stamp<self> {
    function stamp(value: self, comptime tag: word) returns (word);
}

impl Stamp<storage<array<word>>> {
    function stamp(value: storage<array<word>>, comptime tag: word) returns (word) {
        return tag;
    }
}

contract C {
    stored: array<word>;

    function literalTag() returns (word) {
        return stored.stamp(7);
    }

    function runtimeTag(tag: word) returns (word) {
        return stored.stamp(tag);
    }
}
