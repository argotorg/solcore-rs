pragma solcore;

import * from std;
import * from std.dispatch;

contract Invalid {
    function get() public returns (memory<DynArray<(uint256, uint256)>>) {
        return allocateDynamicArray(@(uint256, uint256), 0);
    }
}
