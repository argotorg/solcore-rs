pragma solcore;

function get(comptime value: word) returns (comptime<(word, word)>) {
    return (value, value);
}

//~ loader: bare
//~ infer: get diagnostics-empty patterns-known
