pragma solcore;

import * from std;
import * from std.dispatch;

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

function unwrapBoxes(value: Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<Box<word>>>>>>>>>>>>>>>>>>>>>>>>) returns (word) {
    match (value) {
        case Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(payload)))))))))))))))))))))))) {
            return payload;
        }
        default {
            return 0;
        }
    }
}

function unwrapMixed(value: Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<Box<Record<Product<Choice<word>>>>>>>>>>>>>>>>>>>>) returns (word) {
    match (value) {
        case Box(Record(Product(_, Choice(Box(Record(Product(_, Choice(Box(Record(Product(_, Choice(Box(Record(Product(_, Choice(Box(Record(Product(_, Choice(payload)))))))))))))))))))) {
            return payload;
        }
        default {
            return 0;
        }
    }
}

function unwrapRecords(value: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>>>>>>>>>>>>>) returns (word) {
    return value.value.value.value.value.value.value.value.value.value.value.value.value.value.value.value.value.value.value.value.value.value.value.value.value;
}

contract NestedLayouts {
    // #[(7) -> 7]
    // #[(123456789) -> 123456789]
    function inlineBoxes(value: uint256) public returns (uint256) {
        let input: word = Typedef.rep(value);
        return uint256(unwrapBoxes(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(Box(input))))))))))))))))))))))))));
    }

    // #[(7) -> 7]
    // #[(123456789) -> 123456789]
    function sequentialBoxes(value: uint256) public returns (uint256) {
        let input: word = Typedef.rep(value);
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
        return uint256(unwrapBoxes(value24));
    }

    // #[(7) -> 7]
    // #[(123456789) -> 123456789]
    function inlineMixed(value: uint256) public returns (uint256) {
        let input: word = Typedef.rep(value);
        return uint256(unwrapMixed(Box(Record(Product(0, Choice(Box(Record(Product(0, Choice(Box(Record(Product(0, Choice(Box(Record(Product(0, Choice(Box(Record(Product(0, Choice(input))))))))))))))))))))));
    }

    // #[(7) -> 7]
    // #[(123456789) -> 123456789]
    function sequentialMixed(value: uint256) public returns (uint256) {
        let input: word = Typedef.rep(value);
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
        return uint256(unwrapMixed(value20));
    }

    // #[(7) -> 7]
    // #[(123456789) -> 123456789]
    function inlineRecords(value: uint256) public returns (uint256) {
        let input: word = Typedef.rep(value);
        return uint256(unwrapRecords(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(Record(input))))))))))))))))))))))))));
    }

    // #[(7) -> 7]
    // #[(123456789) -> 123456789]
    function sequentialRecords(value: uint256) public returns (uint256) {
        let input: word = Typedef.rep(value);
        let value1: Record<word> = Record(input);
        let value2: Record<Record<word>> = Record(value1);
        let value3: Record<Record<Record<word>>> = Record(value2);
        let value4: Record<Record<Record<Record<word>>>> = Record(value3);
        let value5: Record<Record<Record<Record<Record<word>>>>> = Record(value4);
        let value6: Record<Record<Record<Record<Record<Record<word>>>>>> = Record(value5);
        let value7: Record<Record<Record<Record<Record<Record<Record<word>>>>>>> = Record(value6);
        let value8: Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>> = Record(value7);
        let value9: Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>> = Record(value8);
        let value10: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>> = Record(value9);
        let value11: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>> = Record(value10);
        let value12: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>> = Record(value11);
        let value13: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>> = Record(value12);
        let value14: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>>> = Record(value13);
        let value15: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>>>> = Record(value14);
        let value16: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>>>>> = Record(value15);
        let value17: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>>>>>> = Record(value16);
        let value18: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>>>>>>> = Record(value17);
        let value19: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>>>>>>>> = Record(value18);
        let value20: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>>>>>>>>> = Record(value19);
        let value21: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>>>>>>>>>> = Record(value20);
        let value22: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>>>>>>>>>>> = Record(value21);
        let value23: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>>>>>>>>>>>> = Record(value22);
        let value24: Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<Record<word>>>>>>>>>>>>>>>>>>>>>>>> = Record(value23);
        return uint256(unwrapRecords(value24));
    }

}
