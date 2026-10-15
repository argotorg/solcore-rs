import * from std;
import { sload, sstore } from std.opcodes;
import { sender } from context;

export { owner, initOwner, requireOwner, transferOwnership };

// Owner address owned by this module at an ERC-7201 namespaced slot. The
// zero address doubles as the not-yet-initialized state.

function ownerSlot() returns (word) {
    return Typedef.rep(erc7201("token.ownable"));
}

function owner() returns (address) {
    return address(sload(ownerSlot()));
}

function initOwner(who: address) {
    require(owner() == address(0), "already initialized");
    sstore(ownerSlot(), Typedef.rep(who));
}

function requireOwner() {
    require(sender() == owner(), "not the owner");
}

function transferOwnership(to: address) {
    requireOwner();
    sstore(ownerSlot(), Typedef.rep(to));
}
