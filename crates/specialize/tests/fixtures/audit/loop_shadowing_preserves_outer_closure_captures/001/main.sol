pragma solcore;

//~ specialize: loader bare

function main(x: word) returns (word) {
    for (let i: word = 0; primEqWord(i, 0); i = 1) {
        let x: word = 7;
    }
    let get: function () returns (word) = lam () -> word {
        let result: word;
        assembly {
            result := x
        }
        return result;
    };
    return get();
}
