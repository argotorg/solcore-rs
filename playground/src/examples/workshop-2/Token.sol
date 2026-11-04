pragma solcore;

import * from std;
// std.dispatch routes each call to the matching public function.
import * from std.dispatch;

// Every change to balances and supply is one Change value.
// Mint credits an account, Transfer moves tokens between two accounts, and Burn debits an account.
enum Change {
    Mint(address),
    Transfer(address, address),
    Burn(address)
}

// The playground deploys this contract from 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 and runs every test from that address.
// The tests use 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 as a second account.
contract Token {
    balances: mapping(address => uint256);
    supply: uint256;

    constructor() {
        apply(Change.Mint(msgSender()), uint256(1000));
    }

    // The only place that touches balances and supply.
    // The compiler checks that every kind of Change is handled.
    // Stretch goal: add a fourth variant to Change and see what breaks.
    function apply(change: Change, amount: uint256) {
        match (change) {
            case Change.Mint(to) {
                balances[to] = balances[to] + amount;
                supply = supply + amount;
            }
            // TODO: add the missing cases.
            // The compile error names them one at a time.
        }
    }

    // #[send(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 100)]
    // #[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 5000) -> revert]
    function transfer(to: address, amount: uint256) public {
        apply(Change.Transfer(msgSender(), to), amount);
    }

    // #[send(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 50)]
    function mint(to: address, amount: uint256) public {
        apply(Change.Mint(to), amount);
    }

    // #[send(200)]
    // #[(5000) -> revert]
    function burn(amount: uint256) public {
        apply(Change.Burn(msgSender()), amount);
    }

    // #[(0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266) -> 700]
    // #[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8) -> 150]
    function balanceOf(who: address) public returns (uint256) {
        return balances[who];
    }

    // #[() -> 850]
    function totalSupply() public returns (uint256) {
        return supply;
    }
}
