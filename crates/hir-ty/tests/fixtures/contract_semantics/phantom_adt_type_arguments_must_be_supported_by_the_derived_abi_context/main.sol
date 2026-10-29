pragma solcore;

import * from std;
import * from std.Generic;
import * from std.ABIGeneric;

enum Phantom<a> {
    Phantom(uint256)
}

contract PhantomAbi {
    function take(value: Phantom<mapping(uint256 => uint256)>) public returns (uint256) {
        return uint256(0);
    }
    function make() public returns (Phantom<mapping(uint256 => uint256)>) {
        return Phantom(uint256(0));
    }
}
