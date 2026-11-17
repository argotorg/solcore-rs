pragma solcore;

function c(comptime x: word) returns (word) {
    return x;
}

contract K {
    function main() public returns (word) {
        let x: word;
        assembly {
            x := calldataload(0)
        }
        return c(x);
    }
}
