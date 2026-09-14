// Candidate standard-library inventory: this module should disappear once
// std provides a reentrancy guard.
import * from std;
import {tload, tstore} from std.opcodes;

export { withLock };

// The lock lives at an ERC-7201 namespaced transient slot, so it clears
// itself at the end of the transaction.
function lockSlot() returns (word) {
    return Typedef.rep(erc7201("vault.reentrancy"));
}

// Runs the body while holding the lock. A nested call reverts.
function withLock<r>(body: function() returns (r)) returns (r) {
    require(tload(lockSlot()) == 0, "reentrant call");
    tstore(lockSlot(), 1);
    let result = body();
    tstore(lockSlot(), 0);
    return result;
}
