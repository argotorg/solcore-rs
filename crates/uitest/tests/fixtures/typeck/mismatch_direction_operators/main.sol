pragma solcore;

trait Add<a> {
    function add(x: a, y: a) returns (bool);
}
impl Add<word> {
    function add(x: word, y: word) returns (bool) {
        return true;
    }
}

trait BitNot<a> {
    function bnot(x: a) returns (bool);
}
impl BitNot<word> {
    function bnot(x: word) returns (bool) {
        return true;
    }
}

function binary_result(x: word) {
    let y: word = x + x;
}

function unary_result(x: word) {
    let y: word = ~x;
}
