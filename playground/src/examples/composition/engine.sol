// The composition machinery shared by every vault in main.sol. In Classic
// Solidity this role is played by base contracts and virtual functions; the
// crossings then need override(...) lists. Traits have one impl per type,
// so there is nothing to disambiguate.
import * from std;
import { sender } from context;

export {
    TxnContext,
    FeePolicy,
    settle,
    Direct(*),
    Signed(*),
    NoFee(*),
    FlatFee(*),
    BasisFee(*),
    Stacked(*)
};

// Axis one: where does the acting address come from?
trait TxnContext<c> {
    function originator(ctx: c) returns (address);
}

// A direct call: the transaction caller acts for themselves.
enum Direct {
    Direct
}

impl TxnContext<Direct> {
    function originator(ctx: Direct) returns (address) {
        return sender();
    }
}

// A relayed call: the acting address is recovered from a signature over
// the digest the relayer hands in alongside it.
enum Signed {
    Signed(bytes32, uint256, bytes32, bytes32)
}

impl TxnContext<Signed> {
    function originator(ctx: Signed) returns (address) {
        match (ctx) {
            case Signed(digest, v, r, s) {
                // std's ecrecover reverts on malleable, failed, or
                // zero-address recovery, so this can never return a bogus
                // signer. The invariant lives in one place.
                return ecrecover(digest, v, r, s);
            }
        }
    }
}

// Axis two: how much of a deposit is credited?
trait FeePolicy<f> {
    function afterFee(policy: f, amount: uint256) returns (uint256);
}

enum NoFee {
    NoFee
}

impl FeePolicy<NoFee> {
    function afterFee(policy: NoFee, amount: uint256) returns (uint256) {
        return amount;
    }
}

enum FlatFee {
    FlatFee(uint256)
}

impl FeePolicy<FlatFee> {
    function afterFee(policy: FlatFee, amount: uint256) returns (uint256) {
        match (policy) {
            case FlatFee(fee) {
                if (amount > fee) {
                    return amount - fee;
                }
                return uint256(0);
            }
        }
    }
}

// A percentage fee in basis points (parts per ten thousand).
enum BasisFee {
    BasisFee(uint256)
}

impl FeePolicy<BasisFee> {
    function afterFee(policy: BasisFee, amount: uint256) returns (uint256) {
        match (policy) {
            case BasisFee(bps) {
                // std uint256 arithmetic is unchecked today: the multiply
                // wraps for amounts above 2^256 / bps.
                return amount - amount * bps / uint256(10000);
            }
        }
    }
}

// Policies compose as values, applied left to right. The order is the
// expression written at the use site, not the C3 linearization of an
// inheritance list.
enum Stacked<f, g> {
    Stacked(f, g)
}

impl<f, g> FeePolicy<Stacked<f, g>> where f: FeePolicy, g: FeePolicy {
    function afterFee(policy: Stacked<f, g>, amount: uint256) returns (uint256) {
        match (policy) {
            case Stacked(first, second) {
                return FeePolicy.afterFee(second, FeePolicy.afterFee(first, amount));
            }
        }
    }
}

// The engine, written once for every context and fee policy: who gets
// credited with how much. Storage stays with each contract.
function settle<c, f>(ctx: c, policy: f, amount: uint256) returns ((address, uint256))
where c: TxnContext, f: FeePolicy
{
    return (TxnContext.originator(ctx), FeePolicy.afterFee(policy, amount));
}
