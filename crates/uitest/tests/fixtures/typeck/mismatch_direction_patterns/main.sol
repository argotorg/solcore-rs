pragma solcore;

enum Box {
    Box(word)
}
enum Empty {
    Empty
}

function constructor_pattern(x: bool) {
    match (x) {
        case Box(y) {}
        case _ {}
    }
}

function nullary_pattern(x: bool) {
    match (x) {
        case Empty {}
        case _ {}
    }
}

function numeric_pattern(x: bool) {
    match (x) {
        case 1 {}
        case _ {}
    }
}

function hex_pattern(x: bool) {
    match (x) {
        case 0x1 {}
        case _ {}
    }
}

function tuple_pattern(x: bool) {
    match (x) {
        case (a, b) {}
        case _ {}
    }
}

function constructor_argument_pattern(x: Box) {
    match (x) {
        case Box(true) {}
        case _ {}
    }
}
