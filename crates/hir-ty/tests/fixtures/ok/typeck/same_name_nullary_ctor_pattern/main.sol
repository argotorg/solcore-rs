pragma solcore;

enum thing {
    thing
}
enum m {
    m,
    k
}

function pickThing(t: thing) returns (word) {
    match (t) {
        case thing {
            return 7;
        }
    }
}

function pickM(x: m) returns (word) {
    match (x) {
        case m {
            return 1;
        }
        case m.k {
            return 2;
        }
    }
}

function main() returns (word) {
    return primAddWord(pickThing(thing), pickM(m.k));
}
