import * from std;
import * from std.dispatch;
import { Change, apply } from erc20core;
import { Pausable, pauseState, isPaused, requireNotPaused } from pausable;
import { Capped, cappedAt, requireWithinCap } from capped;
import { Ownable, ownedBy, ownerOf, requireOwner, transferred } from ownable;

// An ERC-20 style token with an owner, pausing and a supply cap.
// Approvals are not implemented.
contract Token {
    ownable: Ownable;
    pausable: Pausable;
    capped: Capped;
    balances: mapping(address => uint256);
    supply: uint256;

    constructor() {
        ownable = ownedBy(msgSender());
        capped = cappedAt(uint256(1000000));
    }

    // Applies a balance change. Reverts while paused or if the new total
    // supply is above the cap. Change balances and supply only through this
    // function.
    function update(change: Change, amount: uint256) {
        requireNotPaused(pausable);
        supply = apply(balances, supply, change, amount);
        requireWithinCap(capped, supply);
    }

    function transfer(to: address, amount: uint256) public {
        update(Change.Transfer(msgSender(), to), amount);
    }

    function mint(to: address, amount: uint256) public {
        requireOwner(ownable);
        update(Change.Mint(to), amount);
    }

    function burn(amount: uint256) public {
        update(Change.Burn(msgSender()), amount);
    }

    function balanceOf(who: address) public returns (uint256) {
        return balances[who];
    }

    function totalSupply() public returns (uint256) {
        return supply;
    }

    function pause() public {
        requireOwner(ownable);
        pausable = pauseState(true);
    }

    function unpause() public {
        requireOwner(ownable);
        pausable = pauseState(false);
    }

    function paused() public returns (bool) {
        return isPaused(pausable);
    }

    function owner() public returns (address) {
        return ownerOf(ownable);
    }

    function transferOwnership(to: address) public {
        ownable = transferred(ownable, to);
    }
}
