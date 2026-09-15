// Candidate standard-library inventory: this module should disappear once
// std provides reentrancy locks.
import * from std;
import {tload, tstore} from std.opcodes;

export { withWriteLock, withReadLock };

// The locks live at ERC-7201 namespaced transient slots. The explicit
// releases below are what allow sequential use within one transaction;
// the end-of-transaction zeroing is only a backstop, since one
// transaction can bundle several logical operations.
//
// Each lock id owns one transient word: bit zero is the write flag and
// every active read adds two. The word is 0 when free, 1 during a write,
// and a larger even number while only reads are running.
function slotFor(lock: uint256) returns (word) {
    return hash2(Typedef.rep(erc7201("vault.reentrancy")), Typedef.rep(lock));
}

// Runs the body while holding the write lock: until it finishes, no
// other entry through this lock id is allowed, read or write.
function withWriteLock<r>(lock: uint256, body: function() returns (r)) returns (r) {
    let slot = slotFor(lock);
    require(tload(slot) == 0, "already locked");
    tstore(slot, 1);
    let result = body();
    tstore(slot, 0);
    return result;
}

// Runs the body while holding a read lock: writes are blocked, further
// reads may nest. This is the guard against read-only reentrancy.
function withReadLock<r>(lock: uint256, body: function() returns (r)) returns (r) {
    let slot = slotFor(lock);
    let state = tload(slot);
    require(state % 2 == 0, "write in progress");
    tstore(slot, state + 2);
    let result = body();
    tstore(slot, tload(slot) - 2);
    return result;
}
