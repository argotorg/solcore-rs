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
function unwrap(value: Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>>>>>>>>>>>>) returns (word) {
    match (value) {
        case Box(Record(Product(_, Choice(Box(Record(Product(_, Choice(Box(Record(Product(_, Choice(Box(Record(Product(_, Choice(Box(Record(Product(_, Choice(Box(Record(Product(_, Choice(payload)))))))))))))))))))))))) {
            return payload;
        }
        default {
            return 0;
        }
    }
}
function main(input: word) returns (word) {

    return unwrap(Box(Record(Product(0, Choice(Box(Record(Product(0, Choice(Box(Record(Product(0, Choice(Box(Record(Product(0, Choice(Box(Record(Product(0, Choice(Box(Record(Product(0, Choice(input)))))))))))))))))))))))));
}
