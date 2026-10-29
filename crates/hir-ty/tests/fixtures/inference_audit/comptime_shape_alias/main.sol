pragma solcore;

type StaticPair = comptime<(word, word)>;
function get(value: StaticPair) returns (word) {
    match (value) {
        case ((_, tail)) {
            return tail;
        }
    }
}

//~ loader: bare
//~ infer: get diagnostics-empty patterns-known
