pragma solcore;

type Tail = (word, word);

function keep<a>(value: (word, a)) returns ((word, a)) {
    return value;
}

function triple(x: word, y: word, z: word) returns ((word, word, word)) {
    return keep((x, y, z));
}

function inferred(x: word, y: word, z: word) returns ((word, word, word)) {
    let value = keep((x, y, z));
    return value;
}

function aliasTail(value: (word, Tail)) returns (word) {
    match (value) {
        case (x, y, z) {
            return z;
        }
    }
}

function nested(x: word, y: word, z: word) returns ((word, word, word)) {
    return keep((x, (y, z)));
}
