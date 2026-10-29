pragma solcore;

//~ specialize: loader bare
//~ specialize: no-closure-dispatch

function main(n: word) returns (word) {
    let observed: word = 0;
    let get: function () returns (word) = lam () -> word {
        let v: word;
        assembly {
            v := n
        }
        return v;
    };
    match ((true, get)) {
        case ((n, get)) {
            observed = get();
        }
    }
    return primAddWord(n, observed);
}
