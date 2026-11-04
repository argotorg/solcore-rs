pragma solcore;

// Extension points as a trait instead of virtual functions.
// Classic Solidity: override _update in each base contract, then write override(A, B) and rely on C3 linearization to decide the order.
// Core: one trait, one impl per type, and hooks combine as values.
// The order is the expression written in the contract.
import * from std;
import * from std.Generic;
import * from std.StorageGeneric;
import { Change } from change;

export { TransferHook, MaxTransfer(*), Blocked(*), Stacked(*) };

trait TransferHook<h> {
    function check(hook: h, change: Change, amount: uint256);
}

// Rejects transfers above a limit.
// Mints and burns are not limited.
enum MaxTransfer {
    MaxTransfer(uint256)
}

impl TransferHook<MaxTransfer> {
    function check(hook: MaxTransfer, change: Change, amount: uint256) {
        match (hook) {
            case MaxTransfer(limit) {
                match (change) {
                    case Change.Transfer(_, _) {
                        require(amount <= limit, "transfer above limit");
                    }
                    default {
                    }
                }
            }
        }
    }
}

// Rejects transfers to one address.
enum Blocked {
    Blocked(address)
}

impl TransferHook<Blocked> {
    function check(hook: Blocked, change: Change, amount: uint256) {
        match (hook) {
            case Blocked(blocked) {
                match (change) {
                    case Change.Transfer(_, to) {
                        require(to != blocked, "recipient blocked");
                    }
                    default {
                    }
                }
            }
        }
    }
}

// Two hooks run one after the other.
// Works for any pair of hooks, including another Stacked.
enum Stacked<f, g> {
    Stacked(f, g)
}

impl<f, g> TransferHook<Stacked<f, g>> where f: TransferHook, g: TransferHook {
    function check(hook: Stacked<f, g>, change: Change, amount: uint256) {
        match (hook) {
            case Stacked(first, second) {
                TransferHook.check(first, change, amount);
                TransferHook.check(second, change, amount);
            }
        }
    }
}
