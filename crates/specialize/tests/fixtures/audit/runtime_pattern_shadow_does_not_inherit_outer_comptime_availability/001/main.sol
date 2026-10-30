pragma solcore;

//~ specialize: loader bare
//~ specialize: diagnostic comptime-evaluation-failed contains runtime value passed to comptime parameter

enum Box {
    Box(word)
}
function take(comptime value: word) returns (word) {
    return value;
}
function get(value: Box) returns (word) {
    let y: comptime<word> = 3;
    match (value) {
        case (Box(y)) {
            return take(y);
        }
    }
}
function main(value: word) returns (word) {
    return get(Box(value));
}
