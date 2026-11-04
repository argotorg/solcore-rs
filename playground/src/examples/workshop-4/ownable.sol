pragma solcore;

import * from std;
import * from std.Generic;
import * from std.StorageGeneric;

// Exporting Ownable without (*) hides its constructor and fields, so other modules can create and read it only through the functions below.
// Writing Ownable(*) would export them too.
export { Ownable, ownedBy, ownerOf, requireOwner, transferred };

// The owner of a contract.
struct Ownable {
    owner: address;
}

// Sets the first owner.
// Reverts if `state` already has an owner.
function ownedBy(state: Ownable, who: address) returns (Ownable) {
    require(state.owner == address(0), "already owned");
    return Ownable(who);
}

function ownerOf(state: Ownable) returns (address) {
    return state.owner;
}

// Reverts unless the caller is the owner.
function requireOwner(state: Ownable) {
    require(msgSender() == state.owner, "not the owner");
}

// Returns the state with `to` as the owner.
// Reverts unless the caller is the owner.
function transferred(state: Ownable, to: address) returns (Ownable) {
    requireOwner(state);
    return Ownable(to);
}
