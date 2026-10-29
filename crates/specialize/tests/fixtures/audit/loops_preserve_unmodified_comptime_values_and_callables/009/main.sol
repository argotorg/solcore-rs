pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: entry-return 7

function main(flag: bool) returns (word) {
    let get: function () returns (word) = lam () -> word {
        return 7;
    };
    for (; flag;) {
        {
            let seed: word = 8;
            seed = 9;
        }
        break;
    }
    return get();
}
