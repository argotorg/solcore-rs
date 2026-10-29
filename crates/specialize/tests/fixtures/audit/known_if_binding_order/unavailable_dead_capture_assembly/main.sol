pragma solcore;

//~ specialize: loader fixture
//~ specialize: frontend clean
//~ specialize: no-closure-dispatch
//~ specialize: return 7

function pick() returns (function () returns (word)) {
    let x: word;
    return lam () -> word {
        if (true) {
            return 7;
        } else {
            let result: word;
            assembly {
                result := x
            }
            return result;
        }
    };
}
function main() returns (word) {
    let f = pick();
    return f();
}
