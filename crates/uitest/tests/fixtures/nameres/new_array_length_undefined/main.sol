pragma solcore;

// Names in the length of `new T[](n)` resolve like in any other expression.
function allocate(n: word) returns (word) {
    let m = new word[](missingLength);
    return n;
}
