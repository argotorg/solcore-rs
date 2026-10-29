pragma solcore;

function call_argument(g: function (bool) returns (word), x: word) {
    let y: word = g(x);
}

function call_result(g: function (word) returns (bool), x: word) {
    let y: word = g(x);
}

function call_return(g: function (word) returns (bool), x: word) returns (word) {
    return g(x);
}

function comptime_call_argument(comptime g: function (bool) returns (word), x: word) {
    let y: word = g(x);
}

function comptime_call_result(comptime g: function (word) returns (bool), x: word) {
    let y: word = g(x);
}
