pragma solcore;

function inc(x: word) returns (word) {
    let f = lam () {
        return x;
    };
    return f();
}
