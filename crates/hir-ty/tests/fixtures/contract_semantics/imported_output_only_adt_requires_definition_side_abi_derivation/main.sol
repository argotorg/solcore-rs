pragma solcore;

import * from std;
import * from std.dispatch;
import * from model;

contract Imported {
    function make() public returns (Payload) {
        return Payload.Left(uint256(1));
    }
}
