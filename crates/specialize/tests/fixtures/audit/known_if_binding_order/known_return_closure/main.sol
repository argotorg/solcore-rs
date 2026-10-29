pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch
//~ specialize: return 7

function c() returns (function () returns (word)) {
    if (true) {
        return lam () -> word {
            return 7;
        };
    } else {
        let unused: word = 1;
        return lam () -> word {
            return 5;
        };
    }
    return lam () -> word {
        return 0;
    };
}
function main() returns (word) {
    let f = c();
    return f();
}
