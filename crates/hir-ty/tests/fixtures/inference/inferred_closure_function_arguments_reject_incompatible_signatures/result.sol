pragma solcore;

function apply(f: function (word) returns (word), x: word) returns (word) {
    return f(x);
}
function main() returns (word) {
    let f = lam (x: word) -> bool {
        return true;
    };
    return apply(f, 42);
}
