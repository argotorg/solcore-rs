pragma solcore;

import * from std;

export { Update(*), settleBalances, supplyAfter, settleMembers };

// A change to a ledger: a mint to an account, a transfer between two
// accounts, or a burn from an account, each with an amount.
enum Update {
    Mint(address, uint256),
    Transfer(address, address, uint256),
    Burn(address, uint256)
}

// Applies update to token balances in place.
// Reverts if the sender of a transfer or burn holds less than the amount.
function settleBalances(balances: storage<mapping(address => uint256)>, update: Update) {
    match (update) {
        case Update.Mint(to, amount) {
            balances[to] = balances[to] + amount;
        }
        case Update.Transfer(from, to, amount) {
            require(balances[from] >= amount, "insufficient balance");
            balances[from] = balances[from] - amount;
            balances[to] = balances[to] + amount;
        }
        case Update.Burn(from, amount) {
            require(balances[from] >= amount, "insufficient balance");
            balances[from] = balances[from] - amount;
        }
    }
}

// Returns the total token supply after update.
function supplyAfter(supply: uint256, update: Update) returns (uint256) {
    match (update) {
        case Update.Mint(_, amount) {
            return supply + amount;
        }
        case Update.Transfer(_, _, _) {
            return supply;
        }
        case Update.Burn(_, amount) {
            return supply - amount;
        }
    }
}

// Applies update to memberships in place, ignoring the amount.
// A mint adds a member, a transfer moves a membership to another account,
// and a burn removes a member.
// Reverts if a mint or transfer targets a member, or if a transfer or burn
// starts from an account that is not a member.
function settleMembers(members: storage<mapping(address => bool)>, update: Update) {
    match (update) {
        case Update.Mint(to, _) {
            require(!members[to], "already a member");
            members[to] = true;
        }
        case Update.Transfer(from, to, _) {
            require(members[from], "not a member");
            require(!members[to], "already a member");
            members[from] = false;
            members[to] = true;
        }
        case Update.Burn(from, _) {
            require(members[from], "not a member");
            members[from] = false;
        }
    }
}
