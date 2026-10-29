pragma solcore;

function identity<a>(value: a) returns (a) {
    return value;
}

function invoke<f>(callback: f, x: word) returns (word)
where f: invokable<word, word> {
    return invokable.invoke(callback, x);
}

function main(x: word) returns (word) {
    let f = lam (y: word) -> word {
        return y;
    };
    let callback: function (word) returns (word) = f;
    let g = identity(f);
    invoke(g, x);
    return invoke(f, callback(x));
}
