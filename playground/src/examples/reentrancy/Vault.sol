import * from std;
import * from std.dispatch;
import {sender} from context;
import {withWriteLock, withReadLock} from reentrancy;

// The lock id for the balances state, hash-derived so copies of this
// pattern get distinct ids. Other state would get its own id.
function balancesLock() returns (uint256) {
    return uint256(Typedef.rep(erc7201("vault.balancesLock")));
}

// The vault tracks credits only; token custody is omitted.
contract Vault {
    balances : mapping(address => uint256);

    function deposit(amount: uint256) public {
        withWriteLock(balancesLock(), lam () -> () {
            balances[sender()] = balances[sender()] + amount;
        });
    }

    function withdraw(amount: uint256) public returns (uint256) {
        return withWriteLock(balancesLock(), lam () -> uint256 {
            require(balances[sender()] >= amount, "insufficient balance");
            balances[sender()] = balances[sender()] - amount;
            return amount;
        });
    }

    function balanceOf(who: address) public returns (uint256) {
        return withReadLock(balancesLock(), lam () -> uint256 {
            return balances[who];
        });
    }

    // Read locks nest: this takes one and calls the read-locked balanceOf.
    function totalOf(a: address, b: address) public returns (uint256) {
        return withReadLock(balancesLock(), lam () -> uint256 {
            return balanceOf(a) + balanceOf(b);
        });
    }

    // Entering the write lock twice in one transaction reverts.
    function reenter() public returns (uint256) {
        return withWriteLock(balancesLock(), lam () -> uint256 {
            return withdraw(uint256(0));
        });
    }

    // Reading while a write is in progress reverts: the read-only
    // reentrancy case.
    function readDuringWrite() public returns (uint256) {
        return withWriteLock(balancesLock(), lam () -> uint256 {
            return balanceOf(sender());
        });
    }
}
