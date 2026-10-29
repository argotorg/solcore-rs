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
    let value1: Choice<word> = Choice(input);
    let value2: Product<Choice<word>> = Product(0, value1);
    let value3: Record<Product<Choice<word>>> = Record(value2);
    let value4: Box<Record<Product<Choice<word>>>> = Box(value3);
    let value5: Choice<Box<Record<Product<Choice<word>>>>> = Choice(value4);
    let value6: Product<Choice<Box<Record<Product<Choice<word>>>>>> = Product(0, value5);
    let value7: Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>> = Record(value6);
    let value8: Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>> = Box(value7);
    let value9: Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>> = Choice(value8);
    let value10: Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>> = Product(0, value9);
    let value11: Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>> = Record(value10);
    let value12: Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>> = Box(value11);
    let value13: Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>> = Choice(value12);
    let value14: Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>> = Product(0, value13);
    let value15: Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>>> = Record(value14);
    let value16: Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>>>> = Box(value15);
    let value17: Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>>>>> = Choice(value16);
    let value18: Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>>>>>> = Product(0, value17);
    let value19: Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>>>>>>> = Record(value18);
    let value20: Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>>>>>>>> = Box(value19);
    let value21: Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>>>>>>>>> = Choice(value20);
    let value22: Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>>>>>>>>>> = Product(0, value21);
    let value23: Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>>>>>>>>>>> = Record(value22);
    let value24: Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>>>>>>>>>>>> = Box(value23);

    return unwrap(value24);
}
