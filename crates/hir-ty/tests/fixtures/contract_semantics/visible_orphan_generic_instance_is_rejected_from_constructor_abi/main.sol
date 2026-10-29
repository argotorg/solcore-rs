pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from model;

pragma no-generic-instance-for Payload;

impl Generic<Payload, word> {}

contract C {
    constructor(payload: Payload) {}
    function roundtrip(payload: Payload) public returns (Payload) {
        return payload;
    }
}
