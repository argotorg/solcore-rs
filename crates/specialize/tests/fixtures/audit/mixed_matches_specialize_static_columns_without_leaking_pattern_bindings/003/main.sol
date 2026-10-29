pragma solcore;

//~ specialize: loader bare

enum Choice {
    Good(word),
    Bad(word)
}
enum Box<a> {
    Box(a)
}
function take(comptime value: word) returns (word) {
    return value;
}
function main(flag: bool, input: word) returns (word) {
    let value: word = input;
    let get: function () returns (word) = lam () -> word {
        return value;
    };
    match (Box(get), flag) {
        case (Box(invoke), value) {
            return invoke();
        }
    }
}
