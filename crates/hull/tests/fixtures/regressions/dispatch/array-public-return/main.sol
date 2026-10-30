pragma solcore;

import * from std;
import * from std.dispatch;

contract PublicArray {
    constructor() {}

    function values() public returns (memory<DynArray<uint256>>) {
        return [1, 2, 3];
    }
}
