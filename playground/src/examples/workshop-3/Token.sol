pragma solcore;

import * from std;
// std.dispatch routes each call to the matching public function.
import * from std.dispatch;

// TODO: import what this contract needs from ownable.sol, for example:
// import { Ownable, ownedBy } from ownable;
// The export list at the top of ownable.sol shows the other names.

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
    ownable: Ownable;
    balances: mapping(address => uint256);
    supply: uint256;

    constructor() {
        ownable = ownedBy(ownable, msgSender());
        apply(Change.Mint(msgSender()), uint256(1000));
    }

    // The only place that touches balances and supply.
    // The compiler checks that every kind of Change is handled.
    function apply(change: Change, amount: uint256) {
        match (change) {
            case Change.Mint(to) {
                balances[to] = balances[to] + amount;
                supply = supply + amount;
            }
            case Change.Transfer(from, to) {
                require(balances[from] >= amount, "insufficient balance");
                balances[from] = balances[from] - amount;
                balances[to] = balances[to] + amount;
            }
            case Change.Burn(from) {
                require(balances[from] >= amount, "insufficient balance");
                balances[from] = balances[from] - amount;
                supply = supply - amount;
            }
        }
    }

    // #[send(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 100)]
    // #[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 5000) -> revert]
    function transfer(to: address, amount: uint256) public {
        apply(Change.Transfer(msgSender(), to), amount);
    }

    // #[send(200)]
    // #[(5000) -> revert]
    function burn(amount: uint256) public {
        apply(Change.Burn(msgSender()), amount);
    }

    // #[(0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266) -> 700]
    // #[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8) -> 100]
    function balanceOf(who: address) public returns (uint256) {
        return balances[who];
    }

    // #[() -> 800]
    function totalSupply() public returns (uint256) {
        return supply;
    }

    // #[() -> 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266]
    function owner() public returns (address) {
        return ownerOf(ownable);
    }

    // Makes `to` the owner.
    // Only the owner can call it.
    // The first test makes the second account the owner, so the second test, sent by the deployer, reverts.
    // #[send(0x70997970C51812dc3A010C7d01b50e0d17dc79C8)]
    // #[(0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266) -> revert]
    function transferOwnership(to: address) public {
        // TODO: set ownable to the result of transferred() from ownable.sol.
    }

    // Only the owner can mint.
    // The test runs after the transferOwnership tests above, so the deployer is no longer the owner and the mint reverts.
    // #[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 50) -> revert]
    function mint(to: address, amount: uint256) public {
        // TODO: let only the owner mint, using requireOwner() from ownable.sol.
        apply(Change.Mint(to), amount);
    }

    // Try it: uncomment steal() and compile.
    // ownable.sol exports Ownable without its constructor, so the only ways to set an owner are ownedBy, which works only once, and transferred, which checks the caller.
    // Then change Ownable to Ownable(*) in the export list of ownable.sol and compile again: steal() compiles, and any caller can take ownership.
    // function steal() public {
    //     ownable = Ownable(msgSender());
    // }
}
