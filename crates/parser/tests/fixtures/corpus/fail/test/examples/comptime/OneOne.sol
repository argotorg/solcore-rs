function addWord(l: word, r: word) returns (word) {
    let rw: word;
    assembly {
        rw := add(l, r);
    }
    return rw;
}

function zero() returns (word) {
    0
}
function one() returns (word) {
    addWord(1, zero())
}

contract OneOne {
    function main() returns (word) {
        addWord(one(), one())
    }
}
