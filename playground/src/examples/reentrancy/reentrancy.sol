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
// Each lock id owns two slots: a write flag and a read counter.
function writeSlot(lock: uint256) returns (word) {
    return hash2(Typedef.rep(erc7201("vault.reentrancy")), Typedef.rep(lock));
}

function readSlot(lock: uint256) returns (word) {
    return writeSlot(lock) + 1;
}

// Runs the body while holding the write lock: until it finishes, no
// other entry through this lock id is allowed, read or write.
function withWriteLock<r>(lock: uint256, body: function() returns (r)) returns (r) {
    require(tload(writeSlot(lock)) == 0 && tload(readSlot(lock)) == 0, "already locked");
    tstore(writeSlot(lock), 1);
    let result = body();
    tstore(writeSlot(lock), 0);
    return result;
}

// Runs the body while holding a read lock: writes are blocked, further
// reads may nest. This is the guard against read-only reentrancy.
function withReadLock<r>(lock: uint256, body: function() returns (r)) returns (r) {
    require(tload(writeSlot(lock)) == 0, "write in progress");
    tstore(readSlot(lock), tload(readSlot(lock)) + 1);
    let result = body();
    tstore(readSlot(lock), tload(readSlot(lock)) - 1);
    return result;
}
