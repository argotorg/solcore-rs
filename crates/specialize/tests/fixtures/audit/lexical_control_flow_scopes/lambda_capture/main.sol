pragma solcore;

//~ specialize: loader std
//~ specialize: frontend clean
//~ specialize: return 1

function main() returns (word) {
    let x: word = 1;
    if (true) {
        let x: bool = false;
    }
    let get = lam () -> word {
        return x;
    };
    return get();
}
