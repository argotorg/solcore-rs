pragma solcore;

import * from std;
// std.dispatch routes each call to the matching public function.
import * from std.dispatch;

import { emitTransfer } from events;
import { Ownable, ownedBy, ownerOf, requireOwner, transferred } from ownable;

// Every change to balances and supply is one Change value.
// Mint credits an account, Transfer moves tokens between two accounts, and Burn debits an account.
enum Change {
    Mint(address),
    Transfer(address, address),
    Burn(address)
}

// topic runs at compile time, so the deployed code contains only the resulting 32-byte hash.
// keccakLit only accepts a string known at compile time, such as a string literal.
// The comptime on signature passes that requirement on to everyone who calls topic.
function topic(comptime signature: string) returns (comptime<bytes32>) {
    return bytes32(keccakLit(signature));
}

function logTransfer(from: address, to: address, amount: uint256) {
    emitTransfer(topic("Transfer(address,address,uint256)"), from, to, amount);
}

// Want more? Turn this into a full ERC-20: add name, symbol, decimals, approve, allowance and transferFrom, and make transfer return bool.

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
                logTransfer(address(0), to, amount);
            }
            case Change.Transfer(from, to) {
                require(balances[from] >= amount, "insufficient balance");
                balances[from] = balances[from] - amount;
                balances[to] = balances[to] + amount;
                logTransfer(from, to, amount);
            }
            case Change.Burn(from) {
                require(balances[from] >= amount, "insufficient balance");
                balances[from] = balances[from] - amount;
                supply = supply - amount;
                logTransfer(from, address(0), amount);
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
        ownable = transferred(ownable, to);
    }

    // Only the owner can mint.
    // The test runs after the transferOwnership tests above, so the deployer is no longer the owner and the mint reverts.
    // #[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 50) -> revert]
    function mint(to: address, amount: uint256) public {
        requireOwner(ownable);
        apply(Change.Mint(to), amount);
    }

    // The topic every ERC-20 indexer listens for.
    // Try it: once the test passes, open the Yul tab and search for 1003892871.
    // That is the start of this topic as a decimal number: the compiler put the hash into the code as a constant.
    // #[() -> 0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef]
    function transferTopic() public returns (bytes32) {
        return topic("Transfer(address,address,uint256)");
    }

    // Try it: uncomment anyTopic() and compile.
    // Here the signature depends on an argument, which only exists when the contract is called.
    // function anyTopic(approval: bool) public returns (bytes32) {
    //     let signature: string = "Transfer(address,address,uint256)";
    //     if (approval) {
    //         signature = "Approval(address,address,uint256)";
    //     }
    //     return topic(signature);
    // }
}
