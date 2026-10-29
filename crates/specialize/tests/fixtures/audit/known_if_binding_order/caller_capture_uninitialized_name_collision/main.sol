pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch

function pick(x: word) returns (function () returns (word)) {
    let y: word;
    return lam () -> word {
        return x;
    };
}
function main() returns (word) {
    let y: word;
    assembly {
        y := calldataload(0)
    }
    let f = pick(y);
    return f();
}
