pragma solcore;

import * from std;
// std.dispatch routes each call to the matching public function.
import * from std.dispatch;

// Tests are the #[...] comments above a function, and they run in file order on one deployment.
// #[send(args)] sends a transaction and keeps its changes.
// #[(args) -> result] calls the function and checks the result without keeping changes.

// The playground deploys this contract from 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 and runs every test from that address.
// The tests use 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 as a second account.
// The deployer starts with the whole supply of 1000 tokens.
contract Token {
    balances: mapping(address => uint256);
    supply: uint256;

    constructor() {
        balances[msgSender()] = uint256(1000);
        supply = uint256(1000);
    }

    // Move `amount` from the caller to `to`.
    // Reverts if the caller holds less than `amount`.
    // Try it: once the tests pass, delete the require and run them again.
    // They still pass, because `-` calls Sub.sub from std, which reverts on underflow.
    // #[send(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 100)]
    // #[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 5000) -> revert]
    function transfer(to: address, amount: uint256) public {
        // TODO: check the caller's balance with require, then move amount from the caller (msgSender()) to `to`.
        // Local variables use let, for example `let from = msgSender();`.
        // require takes a condition and an error message: `require(condition, "message");`.
    }

    // #[(0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266) -> 900]
    // #[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8) -> 100]
    function balanceOf(who: address) public returns (uint256) {
        return balances[who];
    }

    // #[() -> 1000]
    function totalSupply() public returns (uint256) {
        return supply;
    }
}
