pragma solcore;

import { Generic } from generic;
import * from abi;
import { Box } from types;

function keepBoxVisible(x: Box<word>) returns (Box<word>) {
    return x;
}
