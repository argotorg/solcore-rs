pragma solcore;

contract StaleCall {
    x: word;

    function setx() returns () {
        x = 8;
    }

    function main() public returns (word) {
        x = 7;
        setx();
        return x;
    }
}
