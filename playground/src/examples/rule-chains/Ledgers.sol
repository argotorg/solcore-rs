pragma solcore;

import * from std;
import * from std.dispatch;
import { Update, settleBalances, supplyAfter, settleMembers } from updates;
import { Override, MaxTransfer, Blocked } from rules;

// Two contracts that run every update through rules from rules.sol.
// Token keeps balances and uses both rules.
// Pass keeps memberships and uses only Blocked.
// The playground deploys each contract from 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 and runs every test from that address.

// An ERC-20 style token.
// Approvals are not implemented.
contract Token {
    balances: mapping(address => uint256);
    supply: uint256;
    blocked: address;

    constructor() {
        // Transfers from or to this account revert.
        blocked = address(0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC);
        update(Update.Mint(msgSender(), uint256(1000)));
    }

    // Runs change through the limit check, then the blocked-account check,
    // then settleBalances, and records the new total supply.
    function update(change: Update) {
        let account = blocked;
        let step = Override.override(Override.override(settleBalances, Blocked(account)), MaxTransfer(uint256(100)));
        step(balances, change);
        supply = supplyAfter(supply, change);
    }

    // #[send(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 100)]
    // #[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8, 101) -> revert]
    // #[(0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC, 1) -> revert]
    function transfer(to: address, amount: uint256) public {
        update(Update.Transfer(msgSender(), to, amount));
    }

    // #[send(200)]
    // #[(5000) -> revert]
    function burn(amount: uint256) public {
        update(Update.Burn(msgSender(), amount));
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
}

// A membership pass that one account at a time can hold.
contract Pass {
    members: mapping(address => bool);
    blocked: address;

    constructor() {
        // Transfers from or to this account revert.
        blocked = address(0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC);
        update(Update.Mint(msgSender(), uint256(1)));
    }

    // Runs change through the blocked-account check, then settleMembers.
    function update(change: Update) {
        let account = blocked;
        let step = Override.override(settleMembers, Blocked(account));
        step(members, change);
    }

    // Moves the caller's pass to `to`.
    // #[(0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC) -> revert]
    // #[send(0x70997970C51812dc3A010C7d01b50e0d17dc79C8)]
    // #[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8) -> revert]
    function transfer(to: address) public {
        update(Update.Transfer(msgSender(), to, uint256(1)));
    }

    // #[(0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266) -> false]
    // #[(0x70997970C51812dc3A010C7d01b50e0d17dc79C8) -> true]
    function isMember(who: address) public returns (bool) {
        return members[who];
    }
}
