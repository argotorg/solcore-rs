pragma solcore;

contract Token {
    constructor(amount: word) payable {}

    function hidden(x: word) returns (word) {
        return x;
    }

    function pay(to: word) public payable returns ((word, bool)) {
        return (to, true);
    }

    fallback() payable {}
}
