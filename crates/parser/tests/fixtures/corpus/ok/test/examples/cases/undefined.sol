pragma solcore;

function undefined<any>() returns (any) {
    assembly {
        revert(0, 0)
    }
}

function useWord(w: word) {}

contract Magic {
    function main() public {
        useWord(undefined());
    }
}
