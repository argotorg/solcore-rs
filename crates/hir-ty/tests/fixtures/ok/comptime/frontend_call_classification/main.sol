pragma solcore;

function id(x: word) returns (word) {
    return x;
}

function id_ct(x: word) returns (comptime<word>) {
    let y: comptime<word> = id(x);
    return id(x);
}
