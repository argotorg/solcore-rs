pragma solcore;

import * from std;
import * from std.dispatch;
import { Box } from types;

// The concrete storage instance belongs to Box's definition module. A
// consumer only needs the ordinary storage classes; it need not import
// Generic, StorageDeriving, or the structural implementation instances.
contract C {
    value: Box<uint256>;

    constructor() {}
}
