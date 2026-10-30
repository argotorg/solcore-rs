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
    match (Choice.Good(3), flag) {
        case (Choice.Bad(_), _) {
            return 9;
        }
        case (Choice.Good(inner), selected) {
            inner = primAddWord(inner, ((selected) ? 1 : 2));
            return inner;
        }
    }
}
