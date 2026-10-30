pragma solcore;

import { wrapper, boxed } from lib;

// Same-name constructors from a selective import stay legal unqualified in
// both pattern and expression position.
function unwrap(u: wrapper) returns (word) {
    match (u) {
        case wrapper(w) {
            return w;
        }
    }
}

function rebox(b: boxed) returns (boxed) {
    match (b) {
        case boxed(w) {
            return boxed(w);
        }
    }
}

function main() returns (word) {
    return unwrap(wrapper(3));
}
