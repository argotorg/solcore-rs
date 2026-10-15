import * from std;
import * from std.dispatch;
import { Box } from types;

// Importing an ordinary Generic ADT does not retroactively create storage
// instances when its definition module never enabled StorageGeneric.
contract C {
    value: Box<uint256>;

    constructor() {}
}
