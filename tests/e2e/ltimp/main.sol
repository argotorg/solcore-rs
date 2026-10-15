import * from std;
import * from std.dispatch;

import { ltproxy } from ltproxy;

contract LtImp {
    // #[() -> true]
    function run() public returns (bool) {
        ltproxy()
    }
}
