pragma solcore;

//~ layout: finite-nonempty
enum Box<a> {
    Box(a)
}
enum Choice<a> {
    Empty,
    Choice(a)
}
enum Product<a> {
    Product(word, a)
}
struct Record<a> {
    value: a;
}
function unwrap(value: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>>>>>>>>>>>>) returns (word) {
    match (value) {
        case Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(payload)))))))))))))))))))))))) {
            return payload;
        }
        default {
            return 0;
        }
    }
}
function main(input: word) returns (word) {

    return unwrap(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(input)))))))))))))))))))))))));
}
