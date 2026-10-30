pragma solcore;

//~ specialize: loader bare

enum Box<a> {
    Box(a)
}
function main(flag: bool) returns (bool) {
    let known: word = 3;
    match ((known, true)) {
        case ((value, value)) {
            value = flag;
            return value;
        }
    }
}
