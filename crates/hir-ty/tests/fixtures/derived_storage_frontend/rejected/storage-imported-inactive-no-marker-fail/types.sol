pragma solcore;

import * from std;
import * from std.Generic;

export { Box(*) };

enum Box<a> {
    Box(a)
}
