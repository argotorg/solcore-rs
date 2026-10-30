pragma solcore;

//~ specialize: loader bare
//~ specialize: return 5

function compute() returns (word) {
    let x: word = 1;
    {
        if (true) {
            let x: word = 4;
        } else {}
        x = 5;
    }
    return x;
}
function main() returns (word) {
    return compute();
}
