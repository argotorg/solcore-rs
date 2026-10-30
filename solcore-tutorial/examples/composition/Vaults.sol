pragma solcore;

import * from std;
import * from std.dispatch;
import { settle, Direct, Signed, NoFee, FlatFee, BasisFee, Stacked } from engine;

// Three deployable vaults sharing the engine module, each binding its own
// context and fee choice. This file is the counterpart of an inheritance
// diamond's leaf contracts; note what is absent: override lists, super
// chains, and linearization order. The trade: each leaf repeats its two
// storage lines, because storage stays contract-scoped. The vaults track
// credits only; token custody is omitted.

contract VaultDirect {
    balances: mapping(address => uint256);

    function deposit(amount: uint256) public {
        match (settle(Direct, NoFee, amount)) {
            case (who, credited) {
                balances[who] = balances[who] + credited;
            }
        }
    }

    function balanceOf(who: address) public returns (uint256) {
        return balances[who];
    }
}

// Gasless deposits: anyone may relay the call, and the credited account is
// recovered from a signature over the amount. Real code would also bind a
// nonce, the chain id, and the vault address into the digest to prevent
// replay.
contract VaultGasless {
    balances: mapping(address => uint256);
    collected: uint256;
    flatFee: uint256;

    constructor(fee: uint256) {
        flatFee = fee;
    }

    function depositFor(amount: uint256, v: uint256, r: bytes32, s: bytes32) public {
        let digest = bytes32(hash1(Num.toWord(amount)));
        match (settle(Signed(digest, v, r, s), FlatFee(flatFee), amount)) {
            case (who, credited) {
                balances[who] = balances[who] + credited;
                collected = collected + (amount - credited);
            }
        }
    }

    function balanceOf(who: address) public returns (uint256) {
        return balances[who];
    }

    function feesCollected() public returns (uint256) {
        return collected;
    }
}

// Stacked fees: a protocol fee in basis points, then a flat tip, ordered by
// the expression below.
contract VaultPremium {
    balances: mapping(address => uint256);

    function deposit(amount: uint256) public {
        let policy = Stacked(BasisFee(uint256(30)), FlatFee(uint256(2)));
        match (settle(Direct, policy, amount)) {
            case (who, credited) {
                balances[who] = balances[who] + credited;
            }
        }
    }

    function balanceOf(who: address) public returns (uint256) {
        return balances[who];
    }
}
