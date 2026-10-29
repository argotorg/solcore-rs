pragma solcore;

function main() returns (word) {
    let x: word;
    assembly {
        x := "a\n\t\"\\b"
    }
    return x;
}
