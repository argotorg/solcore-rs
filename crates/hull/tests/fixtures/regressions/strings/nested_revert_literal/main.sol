pragma solcore;

import * from std;
import * from std.dispatch;

contract C {
    fallback() {
        let raw: word;
        assembly {
            raw := callvalue()
        }
        let result: () = ((raw == 0) ? revertLit("nested") : ());
        return result;
    }
}
