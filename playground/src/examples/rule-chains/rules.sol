pragma solcore;

import * from std;
import { Update } from updates;

export { Override, MaxTransfer(*), Blocked(*) };

// override returns a function that checks an operation against context and
// then passes the state and the operation to next.
// The returned function works for any state type.
trait Override<context, operation> {
    function override<state>(next: function (storage<state>, operation) returns (()), context: context) returns (function (storage<state>, operation) returns (()));
}

// Rejects transfers above limit.
// Mints and burns are not limited.
struct MaxTransfer {
    limit: uint256;
}

impl Override<MaxTransfer, Update> {
    function override<state>(next: function (storage<state>, Update) returns (()), context: MaxTransfer) returns (function (storage<state>, Update) returns (())) {
        return lam (balances: storage<state>, update: Update) -> () {
            match (context, update) {
                case (MaxTransfer(limit), Update.Transfer(_, _, amount)) {
                    require(amount <= limit, "transfer exceeds limit");
                }
                default {
                }
            }
            next(balances, update);
        };
    }
}

// Rejects transfers from or to account.
struct Blocked {
    account: address;
}

impl Override<Blocked, Update> {
    function override<state>(next: function (storage<state>, Update) returns (()), context: Blocked) returns (function (storage<state>, Update) returns (())) {
        return lam (balances: storage<state>, update: Update) -> () {
            match (context, update) {
                case (Blocked(account), Update.Transfer(from, to, _)) {
                    require(from != account, "sender is blocked");
                    require(to != account, "recipient is blocked");
                }
                default {
                }
            }
            next(balances, update);
        };
    }
}
