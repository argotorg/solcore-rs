pragma solcore;

function identity<a>(value: a) returns (a) {
    return value;
}

function main(x: word) returns (function (word) returns (word)) {
    let f = lam (y: word) -> word {
        return x;
    };
    return identity(f);
}

function indirect(x: word) returns (function (word) returns (word)) {
    let f = lam (y: word) -> word {
        return x;
    };
    let get = lam () {
        return f;
    };
    return get();
}
