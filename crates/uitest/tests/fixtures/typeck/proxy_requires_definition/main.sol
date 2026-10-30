pragma solcore;

function value() returns (word) {
    return @word;
}

function inferred() {
    let proxy = @word;
}
