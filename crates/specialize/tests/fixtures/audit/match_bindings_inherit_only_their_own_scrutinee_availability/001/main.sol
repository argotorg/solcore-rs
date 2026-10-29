pragma solcore;

//~ specialize: loader bare
//~ specialize: return 3

enum Box<a> {
    Box(a)
}
function take(comptime value: word) returns (word) {
    return value;
}
function get<a>(comptime x: Box<a>, flag: bool) returns (a) {
    match (x, flag) {
        case (Box(y), x) {
            return y;
        }
    }
}
function choose(comptime x: Box<word>, flag: bool) returns (word) {
    match (x, flag) {
        case (Box(y), x) {
            return take(y);
        }
    }
}
function main() returns (word) {
    return take(get(Box(choose(Box(3), true)), false));
}
