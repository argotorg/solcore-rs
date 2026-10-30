pragma solcore;

import * from std;
import * from std.dispatch;
import { setValue } from lib;

contract C {
    function setValue(x: uint256) public {
        setValue(x);
    }
}

trait Marker<T> {
    function mark(x: T) returns (word);
}

impl Marker<word> {
    function mark(x: word) returns (word) {
        return 1;
    }
}

impl Marker<word> {
    function mark(x: word) returns (word) {
        return 2;
    }
}
