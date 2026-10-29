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
    let seed: comptime<word> = 3;
    match (Choice.Good(seed), flag) {
        case (Choice.Bad(_), _) {
            return take(9);
        }
        case (Choice.Good(inner), seed) {
            return take(inner);
        }
    }
}
