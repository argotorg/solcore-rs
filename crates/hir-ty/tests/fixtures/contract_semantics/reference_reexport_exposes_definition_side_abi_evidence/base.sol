pragma solcore;

import * from std;
import * from std.Generic;
import * from std.ABIGeneric;
export { Payload(*) };

enum Payload {
    Left(uint256),
    Right(uint256)
}
