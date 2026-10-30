pragma solcore;

contract Other {
    enum Hidden {
        value
    }
}
function main(x: word) returns (word) {
    match (x) {
        case value {
            return value;
        }
    }
}

//~ loader: bare
//~ nameres-empty
//~ infer: main diagnostics-empty
