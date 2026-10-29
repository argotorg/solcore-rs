import * from std;
import * from std.Generic;
import * from std.StorageGeneric;

// Pausable is exported without its constructor and fields: other modules
// create and read it only through the functions below.
export { Pausable, pauseState, isPaused, requireNotPaused };

// Whether the contract is paused. A storage field that was never assigned
// reads as not paused.
struct Pausable {
    paused: bool;
}

function pauseState(flag: bool) returns (Pausable) {
    return Pausable(flag);
}

function isPaused(state: Pausable) returns (bool) {
    return state.paused;
}

// Reverts while paused.
function requireNotPaused(state: Pausable) {
    require(!state.paused, "paused");
}
