pragma solcore;

//~ specialize: loader bare
//~ specialize: diagnostic comptime-evaluation-failed

enum Box<a> {
    Box(a)
}
function take(comptime value: word) returns (word) {
    return value;
}
function main(input: word) returns (word) {
    let boxed = Box((lam () -> word {
                return input;
            }, input));
    match (boxed) {
        case (Box((invoke, payload))) {
            return take(payload);
        }
    }
}
