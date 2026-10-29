pragma solcore;

import lib;

function unwrap(u: lib.wrapper) returns (word) {
    match (u) {
        case wrapper(w) {
            return w;
        }
    }
}

function main() returns (word) {
    return unwrap(wrapper(3));
}
