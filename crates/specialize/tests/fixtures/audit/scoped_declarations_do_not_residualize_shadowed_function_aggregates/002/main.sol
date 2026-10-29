pragma solcore;

//~ specialize: loader bare

function identity(value: word) returns (word) {
    return value;
}
function main(flag: bool, value: word) returns (word) {
    let zero: word = 0;
    match ((identity, zero)) {
        case (bundle) {
            for (let i: word = 0; primEqWord(i, 0); i = 1) {
                let bundle: word = value;
                return bundle;
            }
            return value;
        }
    }
}
