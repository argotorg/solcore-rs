pragma solcore;

import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Box<a> {
    Box(a)
}
enum Node {
    Node(uint256, Node)
}

pragma no-generic-instance-for Manual;
enum Manual {
    Left(uint256),
    Right(uint256)
}
impl Generic<Manual, sum<uint256, uint256>> {}

contract Rejected {
    function boxed(values: calldata<array<Box<uint256>>>) public returns (uint256) {
        return uint256(0);
    }
    function recursive(values: calldata<array<Node>>) public returns (uint256) {
        return uint256(0);
    }
    function manual(values: calldata<array<Manual>>) public returns (uint256) {
        return uint256(0);
    }
}
