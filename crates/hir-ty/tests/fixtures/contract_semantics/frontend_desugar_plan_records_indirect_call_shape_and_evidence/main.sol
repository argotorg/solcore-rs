pragma solcore;

function apply2<c>(f: c, a: word, b: word) returns (word) where c: invokable<pair<word, word>, word> {
    return f(a, b);
}
