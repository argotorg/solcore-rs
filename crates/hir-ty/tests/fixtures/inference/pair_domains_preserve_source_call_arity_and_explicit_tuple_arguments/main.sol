pragma solcore;

function call_zero(f: function () returns (word)) returns (word) {
    return f();
}

function call_pair(f: function (word, bool) returns (word), x: word, y: bool) returns (word) {
    return f(x, y);
}

function call_tuple(f: function ((word, bool)) returns (word), x: (word, bool)) returns (word) {
    return f(x);
}
