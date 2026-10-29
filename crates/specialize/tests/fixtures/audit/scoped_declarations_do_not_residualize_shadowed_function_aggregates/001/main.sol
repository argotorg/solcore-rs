pragma solcore;

//~ specialize: loader bare

function identity(value: word) returns (word) {
    return value;
}
function main(flag: bool, value: word) returns (word) {
    let zero: word = 0;
    match ((identity, zero)) {
        case (bundle) {
            if (flag) {
                let bundle: word = 42;
                bundle = value;
                return bundle;
            } else {}
            return value;
        }
    }
}
