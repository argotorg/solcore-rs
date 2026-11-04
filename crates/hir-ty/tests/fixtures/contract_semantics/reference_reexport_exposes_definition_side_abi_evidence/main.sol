pragma solcore;

import * from std;
import * from std.dispatch;
import { Payload } from api;

contract Reexported {
    function roundtrip(payload: Payload) public returns (Payload) {
        return payload;
    }
}
