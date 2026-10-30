pragma solcore;

import * from std;
import * from std.dispatch;

contract Arrays {
    function addresses() public returns (memory<DynArray<address>>) {
        return allocateDynamicArray(@address, 0);
    }

    function nested() public returns (memory<DynArray<memory<DynArray<address>>>>) {
        return allocateDynamicArray(@memory<DynArray<address>>, 0);
    }

    function strings() public returns (memory<DynArray<memory<string>>>) {
        return allocateDynamicArray(@memory<string>, 0);
    }

    function pair() public returns (uint256, memory<DynArray<bytes4>>) {
        return (uint256(0), allocateDynamicArray(@bytes4, 0));
    }
}
