pragma solcore;

//~ hull: loader memory

function main() returns (word) {
    let values: (word, word) = (7, 0);
    let result: word = 0;
    match (values) {
        case (x, y) {
            result = x;
        }
    }
    let answer: comptime<word> = result;
    return answer;
}
