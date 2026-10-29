pragma solcore;

enum Box {
    Box(word)
}
enum Empty {
    Empty
}

function constructor_return() returns (bool) {
    return Box(1);
}

function constructor_binding() {
    let x: bool = Box(1);
}

function nullary_constructor() {
    let x: bool = Empty();
}

function constructor_argument() {
    let x: Box = Box(true);
}
