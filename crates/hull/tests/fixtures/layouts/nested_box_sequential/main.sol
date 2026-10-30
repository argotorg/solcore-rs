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
    let value1: Box<word> = Box(input);
    let value2: Box<Box<word>> = Box(value1);
    let value3: Box<Box<Box<word>>> = Box(value2);
    let value4: Box<Box<Box<Box<word>>>> = Box(value3);
    let value5: Box<Box<Box<Box<Box<word>>>>> = Box(value4);
    let value6: Box<Box<Box<Box<Box<Box<word>>>>>> = Box(value5);
    let value7: Box<Box<Box<Box<Box<Box<Box<word>>>>>>> = Box(value6);
    let value8: Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>> = Box(value7);
    let value9: Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>> = Box(value8);
    let value10: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>> = Box(value9);
    let value11: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>> = Box(value10);
    let value12: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>> = Box(value11);
    let value13: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>> = Box(value12);
    let value14: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>> = Box(value13);
    let value15: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>>> = Box(value14);
    let value16: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>>>> = Box(value15);
    let value17: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>>>>> = Box(value16);
    let value18: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>>>>>> = Box(value17);
    let value19: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>>>>>>> = Box(value18);
    let value20: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>>>>>>>> = Box(value19);
    let value21: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>>>>>>>>> = Box(value20);
    let value22: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>>>>>>>>>> = Box(value21);
    let value23: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>>>>>>>>>>> = Box(value22);
    let value24: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>>>>>>>>>>>> = Box(value23);

    return unwrap(value24);
}
