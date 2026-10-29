pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch

function main(n: word) returns (word) {
    let value: word = n;
    {
        let value: bool = true;
        let inner: function () returns (bool) = lam () -> bool {
            return value;
        };
        inner();
    }
    let get: function () returns (word) = lam () -> word {
        assembly {
            value := add(value, 1)
        }
        return value;
    };
    return get();
}
