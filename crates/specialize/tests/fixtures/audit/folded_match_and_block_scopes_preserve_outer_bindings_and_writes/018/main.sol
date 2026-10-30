pragma solcore;

//~ specialize: loader bare
//~ specialize: return 8

function compute() returns (word) {
    let x: word = 7;
    {
        let x: bool = true;
    }
    assembly {
        x := add(x, 1)
    }
    return x;
}
function main() returns (word) {
    return compute();
}
