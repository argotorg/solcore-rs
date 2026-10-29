pragma solcore;

type Callback = function (word) returns (word);

function parameter(g: function (word) returns (word), x: word) returns (word) {
    return g[x];
}

function aliased(g: Callback, x: word) returns (word) {
    return g[x];
}

function staged(comptime g: Callback, x: word) returns (word) {
    return g[x];
}

function generic<a>(g: function (a) returns (a), x: a) returns (a) {
    return g[x];
}

function identity(x: word) returns (word) {
    return x;
}

function named(x: word) returns (word) {
    return identity[x];
}

function closure(x: word) returns (word) {
    let g = lam (value: word) -> word {
        return value;
    };
    return g[x];
}

function immediate(x: word) returns (word) {
    return (lam (value: word) -> word {
            return value;
        })[x];
}

function scalar(x: word) {
    let y: word = x[0];
}

function assignment(g: Callback, x: word) {
    g[x] = x;
}

function nested(g: Callback, x: word) returns (word) {
    return g[x][x];
}

function unknown_base(x: word) returns (word) {
    return missing[x];
}

contract C {
    function apply(g: Callback, x: word) returns (word) {
        return g[x];
    }

    function main() public returns (word) {
        return apply(identity, 7);
    }
}
