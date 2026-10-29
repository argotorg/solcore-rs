pragma solcore;

//~ specialize: loader std
//~ specialize: return 3

import * from std;
enum Box<a> {
    Box(a)
}
function take(comptime value: word) returns (word) {
    return value;
}
function get(comptime x: (word, word)) returns (word) {
    match (x) {
        case ((y, _)) {
            return take(y);
        }
    }
}
function main() returns (word) {
    return get((3, 4));
}
