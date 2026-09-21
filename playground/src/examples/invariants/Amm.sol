import * from std;
import * from std.dispatch;
import {Pool, mkPool, reserveX, reserveY, swapXforY, addLiquidity} from pool;

// Outside pool.sol the Pool constructor is not in scope, so no code path
// can put a zero-reserve pool into storage.
contract Amm {
    pool : Pool;

    constructor() {
        pool = mkPool(uint256(10), uint256(1000));
    }

    function swap(amountIn: uint256) public returns (uint256) {
        match (swapXforY(pool, amountIn)) {
            case (next, amountOut) {
                pool = next;
                return amountOut;
            }
        }
    }

    function deposit(dx: uint256, dy: uint256) public {
        pool = addLiquidity(pool, dx, dy);
    }

    function poolX() public returns (uint256) {
        return reserveX(pool);
    }

    function poolY() public returns (uint256) {
        return reserveY(pool);
    }
}
