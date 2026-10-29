pragma solcore;

function f() returns (word) {
    let x = wordToInteger(20);
    return wordFromInteger(x);
}
