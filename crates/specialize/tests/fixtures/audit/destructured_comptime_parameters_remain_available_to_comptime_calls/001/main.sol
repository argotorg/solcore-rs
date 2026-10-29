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
function get(comptime x: Box<word>) returns (word) {
    match (x) {
        case (Box(y)) {
            return take(y);
        }
    }
}
function main() returns (word) {
    return get(Box(3));
}
