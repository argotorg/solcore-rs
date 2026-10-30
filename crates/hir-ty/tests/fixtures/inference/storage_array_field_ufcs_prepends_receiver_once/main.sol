pragma solcore;

import { array, storage, uint256, Array, ArrayPush, Length } from std;

contract C {
    members: array<uint256>;

    function memberCount() returns (uint256) {
        return members.length();
    }

    function append(value: uint256) returns () {
        members.push(value);
        return ();
    }

    function removeLast() returns () {
        members.pop();
        return ();
    }
}
