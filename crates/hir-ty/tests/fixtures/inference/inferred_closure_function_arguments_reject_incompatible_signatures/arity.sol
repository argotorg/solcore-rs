pragma solcore;

function apply(f: function (word) returns (word), x: word) returns (word) {
    return f(x);
}
function main() returns (word) {
    let f = lam () -> word {
        return 1;
    };
    return apply(f, 42);
}
