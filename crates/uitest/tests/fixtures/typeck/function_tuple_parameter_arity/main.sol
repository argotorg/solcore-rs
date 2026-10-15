function invokePair(f: function ((word, word)) returns (word), x: word, y: word) returns (word) {
    return f(x, y);
}

function invokeTuple(f: function ((word, word)) returns (word), x: word, y: word) returns (word) {
    return f((x, y));
}
