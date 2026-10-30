pragma solcore;

function bool_value() returns (bool) {
    return true;
}
function word_value() returns (word) {
    return 0;
}

function direct_call() {
    let x: word = bool_value();
}

function direct_return() returns (word) {
    return bool_value();
}

function opposite_direction() {
    let x: bool = word_value();
}

function indirect_call(g: function () returns (bool)) {
    let x: word = g();
}

function closure_call() {
    let g = lam () {
        return true;
    };
    let x: word = g();
}

function lambda_value() {
    let x: word = lam () {
        return true;
    };
}
