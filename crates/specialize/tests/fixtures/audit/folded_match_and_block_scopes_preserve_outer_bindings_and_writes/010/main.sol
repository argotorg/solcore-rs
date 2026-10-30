pragma solcore;

//~ specialize: loader bare
//~ specialize: return 3

function compute() returns (word) {
    let x: word = 1;
    {
        x = 2;
        {
            let x: word = 4;
            x = 5;
        }
        x = primAddWord(x, 1);
    }
    return x;
}
function main() returns (word) {
    return compute();
}
