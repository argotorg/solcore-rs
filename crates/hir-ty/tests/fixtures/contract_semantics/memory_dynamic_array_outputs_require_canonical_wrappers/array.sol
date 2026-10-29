pragma solcore;

import { memory, address } from std;

enum DynArray<t> {
    Fake(word)
}

contract Fake {
    function get() public returns (memory<DynArray<address>>) {
        let value: memory<DynArray<address>>;
        return value;
    }
}
