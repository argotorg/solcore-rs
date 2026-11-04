pragma solcore;

import * from std;
// std.dispatch routes each call to the matching public function.
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

import { emitTransfer } from events;
import { Change } from change;
import { TransferHook, MaxTransfer, Blocked, Stacked } from hooks;
import { Ownable, ownedBy, ownerOf, requireOwner, transferred } from ownable;

// topic runs at compile time, so the deployed code contains only the resulting 32-byte hash.
// keccakLit only accepts a string known at compile time, such as a string literal.
// The comptime on signature passes that requirement on to everyone who calls topic.
function topic(comptime signature: string) returns (comptime<bytes32>) {
    return bytes32(keccakLit(signature));
}

function logTransfer(from: address, to: address, amount: uint256) {
    emitTransfer(topic("Transfer(address,address,uint256)"), from, to, amount);
}

// The playground deploys this contract from 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 and runs every test from that address.
// The tests use 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 as a second account.
contract Token {
    ownable: Ownable;
    // Change or reorder the hooks here and in the constructor, which builds the same hooks as a value.
    // The rest of the contract stays the same.
    hook: Stacked<MaxTransfer, Blocked>;
    balances: mapping(address => uint256);
    supply: uint256;

    constructor() {
        ownable = ownedBy(ownable, msgSender());
        hook = Stacked(MaxTransfer(uint256(500)), Blocked(address(0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC)));
        apply(Change.Mint(msgSender()), uint256(1000));
    }

    // The only place that touches balances and supply.
    // The compiler checks that every kind of Change is handled.
    function apply(change: Change, amount: uint256) {
        TransferHook.check(hook, change, amount);
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
    // #[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 600) -> revert]
    // #[(0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC, 10) -> revert]
    // The next test sends 600 to the blocked address, which breaks both hooks.
    // MaxTransfer runs first, so the transfer reverts with "transfer above limit", written as hex below.
    // Swap the hooks and the test fails, because Blocked then reverts first with "recipient blocked".
    // #[(0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC, 600) -> revert(0x7472616e736665722061626f7665206c696d6974)]
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
