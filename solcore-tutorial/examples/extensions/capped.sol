import * from std;
import { sload, sstore } from std.opcodes;
import { Option } from option;
import { TransferHook } from hooks;
import { supply } from erc20core;

export { setCap, requireUnderCap, Capped(*) };

// Supply cap owned by this module at an ERC-7201 namespaced slot.

function capSlot() returns (word) {
    return Typedef.rep(erc7201("token.capped"));
}

function setCap(cap: uint256) {
    sstore(capSlot(), Typedef.rep(cap));
}

function requireUnderCap(newSupply: uint256) {
    require(newSupply <= uint256(sload(capSlot())), "cap exceeded");
}

// Transfer hook: minting may not push supply over the cap.
enum Capped {
    Capped
}

impl TransferHook<Capped> {
    function on(hook: Capped, from: Option<address>, to: Option<address>, amount: uint256) {
        match (from) {
            case Option.Some(src) {}
            default {
                requireUnderCap(supply() + amount);
            }
        }
    }
}
