pragma solcore;

import * from std;
import { sstore } from std.opcodes;

export { setValue };

function setValue(x: uint256) {
    sstore(0, Typedef.rep(x));
}
