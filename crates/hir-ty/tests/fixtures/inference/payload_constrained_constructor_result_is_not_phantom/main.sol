pragma solcore;

enum Box<a> {
    Box(a)
}

function unwrap<a>(x: Box<a>) returns (a) {
    match (x) {
        case Box(value) {
            return value;
        }
    }
}

function main() returns (word) {
    return unwrap(Box(42));
}
