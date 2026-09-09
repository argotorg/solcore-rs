import * from std;
import * from std.dispatch;
import {sender} from context;
import {Option} from option;
import {Stacked, NoHook} from hooks;
import {balance, supply, apply} from erc20core;
import {isPaused, setPaused, Pausable} from pausable;
import {setCap, Capped} from capped;
import {owner as storedOwner, initOwner, requireOwner, transferOwnership as setOwner} from ownable;

// A token composed from self-contained feature modules. Each module owns
// its own storage and ships a transfer hook; the contract routes every
// balance change through one update function, where the hook chain is
// declared once. Access control stays on the entry points that need it.
contract Token {
    constructor() {
        initOwner(sender());
        setCap(uint256(1000000));
    }

    // The single choke point: every balance change flows through here,
    // and the ledger will not move balances without a hook chain.
    function update(from: Option<address>, to: Option<address>, amount: uint256) {
        apply(Stacked(Pausable, Capped), NoHook, from, to, amount);
    }

    function transfer(to: address, amount: uint256) public {
        update(Option.Some(sender()), Option.Some(to), amount);
    }

    function mint(to: address, amount: uint256) public {
        requireOwner();
        update(Option.None, Option.Some(to), amount);
    }

    function burn(amount: uint256) public {
        update(Option.Some(sender()), Option.None, amount);
    }

    function balanceOf(who: address) public returns (uint256) {
        return balance(who);
    }

    function totalSupply() public returns (uint256) {
        return supply();
    }

    function pause() public {
        requireOwner();
        setPaused(true);
    }

    function unpause() public {
        requireOwner();
        setPaused(false);
    }

    function paused() public returns (bool) {
        return isPaused();
    }

    function owner() public returns (address) {
        return storedOwner();
    }

    function transferOwnership(to: address) public {
        setOwner(to);
    }
}
