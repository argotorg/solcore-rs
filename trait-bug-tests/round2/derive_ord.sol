pragma solcore;

import * from std;
import * from std.Generic;

// Ord requires Eq, which is not derived.
#[derive(Ord)]
enum E {
    X,
    Y
}
