pragma solcore;

import { DynArray, address } from std;

enum memory<t> {
    Fake(word)
}

contract Fake {
    function get() public returns (memory<DynArray<address>>) {
        let value: memory<DynArray<address>>;
        return value;
    }
}
