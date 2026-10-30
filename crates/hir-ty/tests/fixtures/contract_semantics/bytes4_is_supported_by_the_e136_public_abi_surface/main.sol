pragma solcore;

import * from std;

contract Bytes4Echo {
    function echo(value: bytes4) public returns (bytes4) {
        return value;
    }
}
