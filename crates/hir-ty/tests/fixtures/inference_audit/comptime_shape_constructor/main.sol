pragma solcore;

enum Box {
    Box(word)
}
function get(comptime value: Box) returns (word) {
    match (value) {
        case (.Box(inner)) {
            return inner;
        }
    }
}

//~ loader: bare
//~ infer: get diagnostics-empty patterns-known
