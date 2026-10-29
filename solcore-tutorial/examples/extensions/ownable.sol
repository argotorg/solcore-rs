pragma solcore;

import * from std;
import * from std.Generic;
import * from std.StorageGeneric;

// Ownable is exported without its constructor and fields: other modules
// create and read it only through the functions below.
export { Ownable, ownedBy, ownerOf, requireOwner, transferred };

// The owner of a contract.
struct Ownable {
    owner: address;
}

function ownedBy(who: address) returns (Ownable) {
    return Ownable(who);
}

function ownerOf(state: Ownable) returns (address) {
    return state.owner;
}

// Reverts unless the caller is the owner.
function requireOwner(state: Ownable) {
    require(msgSender() == state.owner, "not the owner");
}

// Returns the state with `to` as the owner. Reverts unless the caller is
// the owner.
function transferred(state: Ownable, to: address) returns (Ownable) {
    requireOwner(state);
    return Ownable(to);
}
