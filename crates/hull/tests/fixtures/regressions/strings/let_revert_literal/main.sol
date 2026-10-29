pragma solcore;

import * from std;
import * from std.dispatch;

contract C {
    fallback() {
        let unreachable: () = revertLit("let-initializer");
        return unreachable;
    }
}
