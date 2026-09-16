import * from std;
import {sload, sstore} from std.opcodes;
import {Option} from option;
import {TransferHook} from hooks;

export { balance, supply, apply };

// Core ledger owned by this module: balances and total supply live at
// ERC-7201 namespaced slots, not in the importing contract. Approvals are
// omitted.

function supplySlot() returns (word) {
    return Typedef.rep(erc7201("token.erc20.supply"));
}

function supply() returns (uint256) {
    return uint256(sload(supplySlot()));
}

function balanceSlot(who: address) returns (word) {
    return hash2(Typedef.rep(erc7201("token.erc20.balances")), Typedef.rep(who));
}

function balance(who: address) returns (uint256) {
    return uint256(sload(balanceSlot(who)));
}

// The only exported way to change balances: the before and after hook
// chains run around the effects, so none of them can be skipped. A side
// with no hooks takes NoHook. from = None mints, to = None burns.
function apply<b, a>(before: b, after: a, from: Option<address>, to: Option<address>, amount: uint256)
    where b: TransferHook, a: TransferHook
{
    TransferHook.on(before, from, to, amount);
    match (from) {
        case Option.None {
            sstore(supplySlot(), Typedef.rep(supply() + amount));
        }
        case Option.Some(from_) {
            require(balance(from_) >= amount, "insufficient balance");
            sstore(balanceSlot(from_), Typedef.rep(balance(from_) - amount));
        }
    }
    match (to) {
        case Option.None {
            sstore(supplySlot(), Typedef.rep(supply() - amount));
        }
        case Option.Some(to_) {
            sstore(balanceSlot(to_), Typedef.rep(balance(to_) + amount));
        }
    }
    TransferHook.on(after, from, to, amount);
}
