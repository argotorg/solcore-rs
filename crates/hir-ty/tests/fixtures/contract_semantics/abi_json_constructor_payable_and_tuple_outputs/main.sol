pragma solcore;

contract Token {
    constructor(amount: word) {}

    function pay(to: word) public payable returns ((word, bool)) {
        return (to, true);
    }
}
