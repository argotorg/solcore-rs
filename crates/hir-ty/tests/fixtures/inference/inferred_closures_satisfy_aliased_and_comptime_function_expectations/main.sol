pragma solcore;

type Callback = function (word) returns (word);

function staged(comptime callback: Callback, x: word) returns (word) {
    return callback(x);
}

function main(x: word) returns (word) {
    let f = lam (y: word) -> word {
        return y;
    };
    let callback: comptime<Callback> = f;
    return staged(callback, x);
}
