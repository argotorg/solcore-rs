pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch

function main(flag: bool, n: word) returns (word) {
    let value: word = n;
    for (; flag;) {
        let value: bool = true;
        break;
    }
    let get: function () returns (word) = lam () -> word {
        assembly {
            value := add(value, 1)
        }
        return value;
    };
    return get();
}
