pragma solcore;

import * from std;
import * from std.dispatch;
import * from model;

contract Imported {
    function roundtrip(payload: Payload) public returns (Payload) {
        return payload;
    }
}
