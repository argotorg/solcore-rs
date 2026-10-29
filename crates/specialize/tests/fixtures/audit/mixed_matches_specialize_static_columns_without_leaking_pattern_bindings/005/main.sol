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
    match (Box(lam () -> word {
                return value;
            }), flag) {
        case (box, value) {
            match (box) {
                case (Box(invoke)) {
                    return invoke();
                }
            }
        }
    }
}
