import * from std;
import { sload, sstore } from std.opcodes;
import { Option } from option;
import { TransferHook } from hooks;

export { isPaused, setPaused, requireNotPaused, Pausable(*) };

// Pause flag owned by this module at an ERC-7201 namespaced slot.

function pausedSlot() returns (word) {
    return Typedef.rep(erc7201("token.pausable"));
}

function isPaused() returns (bool) {
    return sload(pausedSlot()) != 0;
}

function setPaused(flag: bool) {
    if (flag) {
        sstore(pausedSlot(), 1);
    } else {
        sstore(pausedSlot(), 0);
    }
}

function requireNotPaused() {
    require(!isPaused(), "paused");
}

// Transfer hook: no balance may change while paused.
enum Pausable {
    Pausable
}

impl TransferHook<Pausable> {
    function on(hook: Pausable, from: Option<address>, to: Option<address>, amount: uint256) {
        requireNotPaused();
    }
}
