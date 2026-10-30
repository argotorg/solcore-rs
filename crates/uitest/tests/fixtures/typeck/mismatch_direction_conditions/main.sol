pragma solcore;

function if_condition() {
    let x: word;
    if (x) {} else {}
}

function while_condition(x: word) {
    while (x) {}
}

function for_condition(x: word) {
    for (; x;) {}
}

function conditional_expression(x: word) returns (word) {
    return x ? 1 : 2;
}

function literal_condition() {
    if (1) {}
}

function word_value() returns (word) {
    return 0;
}

function call_condition() {
    if (word_value()) {}
}
