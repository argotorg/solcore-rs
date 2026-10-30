pragma solcore;

//~ hull: valid

import * from std;
import * from std.dispatch;

contract C {
    constructor(config: uint256) {
        let saved_config = config;
    }

    function echo(config: uint256) public returns (uint256) {
        return config;
    }
}
