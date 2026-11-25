pragma solcore;
import * from std;
import * from std.dispatch;

// A field that gets a storage pointer is assigned a distinct value here, for the
// test that runs the code and reads the slot that the debug information names.
// `name` is a string: it takes one slot and has no pointer.
contract Slots {
    first: uint256;
    owner: address;
    second: word;
    digest: bytes32;
    name: string;
    balances: mapping(address => uint256);
    marks: mapping(word => word);
    last: uint256;

    function main() public returns (uint256) {
        first = uint256(11);
        owner = address(0x22);
        second = 33;
        digest = bytes32(44);
        balances[address(0x66)] = uint256(66);
        marks[7] = 77;
        last = uint256(55);
        return first;
    }
}

contract AfterBool {
    head: uint256;
    flag: bool;
    tail: uint256;
    marks: mapping(word => word);

    function main() public returns (uint256) {
        head = uint256(1);
        flag = true;
        tail = uint256(3);
        marks[5] = 9;
        return head;
    }
}
