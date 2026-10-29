pragma solcore;

import * from std;

export { Change(*), apply };

// A balance change: a mint to an account, a transfer between two accounts,
// or a burn from an account.
enum Change {
    Mint(address),
    Transfer(address, address),
    Burn(address)
}

// Applies a change of amount to balances and returns the new total supply.
// Reverts if the sender of a transfer or burn holds less than amount.
function apply(
    balances: storage<mapping(address => uint256)>,
    supply: uint256,
    change: Change,
    amount: uint256
) returns (uint256) {
    match (change) {
        case Change.Mint(to) {
            balances[to] = balances[to] + amount;
            return supply + amount;
        }
        case Change.Transfer(from, to) {
            debit(balances, from, amount);
            balances[to] = balances[to] + amount;
            return supply;
        }
        case Change.Burn(from) {
            debit(balances, from, amount);
            return supply - amount;
        }
    }
}

// Subtracts amount from the balance of from. Reverts if from holds less
// than amount.
function debit(
    balances: storage<mapping(address => uint256)>,
    from: address,
    amount: uint256
) {
    require(balances[from] >= amount, "insufficient balance");
    balances[from] = balances[from] - amount;
}
