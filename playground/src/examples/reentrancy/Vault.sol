import * from std;
import * from std.dispatch;
import {sender} from context;
import {withLock} from reentrancy;

// The vault tracks credits only; token custody is omitted.
contract Vault {
    balances : mapping(address => uint256);

    function deposit(amount: uint256) public {
        balances[sender()] = balances[sender()] + amount;
    }

    // The protected body is a lambda: withLock runs it while the lock is
    // held, the counterpart of a nonReentrant modifier.
    function withdraw(amount: uint256) public returns (uint256) {
        return withLock(lam () -> uint256 {
            require(balances[sender()] >= amount, "insufficient balance");
            balances[sender()] = balances[sender()] - amount;
            return amount;
        });
    }

    function balanceOf(who: address) public returns (uint256) {
        return balances[who];
    }

    // Entering the lock twice in one transaction reverts.
    function reenter() public returns (uint256) {
        return withLock(lam () -> uint256 {
            return withdraw(uint256(0));
        });
    }
}
