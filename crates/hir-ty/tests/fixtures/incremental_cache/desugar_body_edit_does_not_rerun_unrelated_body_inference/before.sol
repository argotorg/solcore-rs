pragma solcore;

function choose(b: bool, x: word, y: word) returns (word) {
    let p: (word, bool) = (x, true);
    let selected: word = (b ? x : y);
    match (p) {
        case (head, flag) {
            return selected;
        }
    }
}

function stable(x: word) returns (word) {
    return x;
}
function main() returns (word) {
    return stable(choose(false, 1, 2));
}
