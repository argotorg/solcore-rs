pragma solcore;

trait Add<t> {
    function add(l: t, r: t) returns (t);
}

impl Add<word> {
    function add(l: word, r: word) returns (word) {
        return primAddWord(l, r);
    }
}

function f(x: word) returns (word) {
    return x + 1;
}
