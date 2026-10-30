pragma solcore;

//~ specialize: loader bare

enum Box<a> {
    Box(a)
}
function main(flag: bool) returns (bool) {
    let known: word = 3;
    match (Box((known, true))) {
        case (Box((value, value))) {
            value = flag;
            return value;
        }
    }
}
