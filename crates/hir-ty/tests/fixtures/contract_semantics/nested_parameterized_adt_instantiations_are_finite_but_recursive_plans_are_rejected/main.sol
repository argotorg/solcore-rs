pragma solcore;

import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Box<a> {
    Box(a)
}
enum Node {
    Node(Node)
}

contract Finite {
    function roundtrip(value: Box<Box<uint256>>) public returns (Box<Box<uint256>>) {
        return value;
    }
}
contract Recursive {
    function recursive(value: Node) public returns (Node) {
        return value;
    }
}
