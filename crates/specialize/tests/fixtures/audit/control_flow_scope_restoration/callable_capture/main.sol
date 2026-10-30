pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch

function main(flag: bool, n: word) returns (word) {
    let get: function () returns (word) = lam () -> word {
        return n;
    };
    {
        if (flag) {
            let get: word = 1;
        } else {}
    }
    return get();
}
