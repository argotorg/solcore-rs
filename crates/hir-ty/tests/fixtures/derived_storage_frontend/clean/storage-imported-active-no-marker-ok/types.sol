pragma solcore;

import * from std;
import * from std.Generic;
import * from std.StorageGeneric;

export { Box(*) };

enum Box<a> {
    Box(a)
}
