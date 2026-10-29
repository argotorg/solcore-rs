pragma solcore;

type Predicate = function (word) returns (bool);

function aliased(g: Predicate, x: word) returns (bool) {
    return g(x);
}

function generic<a>(g: function (a) returns (a), x: a) returns (a) {
    return g(x);
}

function staged(comptime g: Predicate, x: word) returns (bool) {
    return g(x);
}
