pragma solcore;

function need(comptime x: word) returns (comptime<word>) {
    return x;
}

function g() returns (comptime<word>) {
    let y: comptime<word> = need(2);
    return y;
}

function f(x: word) returns (comptime<word>) {
    match (x) {
        case comptime 1 {
            return need(2);
        }
        default {
            return 0;
        }
    }
}
