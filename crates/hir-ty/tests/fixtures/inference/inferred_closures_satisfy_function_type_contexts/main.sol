pragma solcore;

function apply(f: function (word) returns (word), x: word) returns (word) {
    return f(x);
}

function direct(x: word) returns (word) {
    let f = lam (y: word) -> word {
        return y;
    };
    return apply(f, x);
}

function indirect(
    callback: function (function (word) returns (word), word) returns (word),
    x: word
) returns (word) {
    let f = lam (y: word) -> word {
        return y;
    };
    return callback(f, x);
}

function assigned(x: word) returns (word) {
    let f = lam (y: word) -> word {
        return y;
    };
    let g = lam (y: word) -> word {
        return x;
    };
    let callback: function (word) returns (word) = f;
    callback = g;
    return callback(x);
}

function returned(x: word) returns (function (word) returns (word)) {
    let f = lam (y: word) -> word {
        return x;
    };
    return f;
}

function conditional(choose: bool, x: word) returns (function (word) returns (word)) {
    let f = lam (y: word) -> word {
        return y;
    };
    let g = lam (y: word) -> word {
        return x;
    };
    return choose ? f : g;
}
