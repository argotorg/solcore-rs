pragma solcore;

import * from std;
import * from std.Generic;
export { Payload(*) };

enum Payload {
    Left(uint256),
    Right(uint256)
}
