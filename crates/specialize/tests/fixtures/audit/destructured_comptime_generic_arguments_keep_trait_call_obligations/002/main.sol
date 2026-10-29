pragma solcore;

//~ specialize: loader bare
//~ specialize: diagnostic comptime-evaluation-failed contains runtime value passed to comptime parameter

enum Box {
    Box(word)
}
function take(comptime value: word) returns (word) {
    return value;
}
function get(comptime value: Box, input: word) returns (word) {
    match (value) {
        case (Box(inner)) {
            inner = input;
            return take(inner);
        }
    }
}
function main(input: word) returns (word) {
    return get(Box(3), input);
}
