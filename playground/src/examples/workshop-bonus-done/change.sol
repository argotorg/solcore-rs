pragma solcore;

import * from std;
import * from std.Generic;
import * from std.StorageGeneric;

export { Change(*) };

// Change has its own module, so that hooks.sol can use it without importing Token.sol.
// Every change to balances and supply is one Change value.
// Mint credits an account, Transfer moves tokens between two accounts, and Burn debits an account.
enum Change {
    Mint(address),
    Transfer(address, address),
    Burn(address)
}
