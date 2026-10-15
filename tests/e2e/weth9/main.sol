import * from std;
import { caller as caller_, callvalue as callvalue_, selfbalance, gas, call } from std.opcodes;
import * from std.dispatch;

// Forward `wad` wei to `dst` via a zero-data CALL and revert on failure.
function sendValue(dst: address, wad: uint256) {
    let ret = call(gas(), Typedef.rep(dst), Typedef.rep(wad), 0, 0, 0, 0);
    require(ret != 0, Error(0x90b8ec18)); // TransferFailed()
}

function caller() returns (address) {
    return address(caller_());
}

function callvalue() returns (uint256) {
    return uint256(callvalue_());
}

// Based on https://github.com/gnosis/canonical-weth/blob/master/contracts/WETH9.sol
// That code is written WITHOUT checked arithmetic.
// #[scenario("weth9")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0xd0e30db0, value = 100, label = "deposit()  value=100") -> success(0x)]
// #[raw(0x70a082310000000000000000000000001212121212121212121212121212120000000012, label = "balanceOf(caller) -> 100") -> success(0x0000000000000000000000000000000000000000000000000000000000000064)]
// #[raw(0x18160ddd, label = "totalSupply() -> 100 (contract ETH balance)") -> success(0x0000000000000000000000000000000000000000000000000000000000000064)]
// #[raw(0xa9059cbb00000000000000000000000070997970c51812dc3a010c7d01b50e0d17dc79c8000000000000000000000000000000000000000000000000000000000000001e, label = "transfer($ANVIL1, 30) -> true") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x70a0823100000000000000000000000070997970c51812dc3a010c7d01b50e0d17dc79c8, label = "balanceOf($ANVIL1) -> 30") -> success(0x000000000000000000000000000000000000000000000000000000000000001e)]
// #[raw(0x70a082310000000000000000000000001212121212121212121212121212120000000012, label = "balanceOf(caller) -> 70") -> success(0x0000000000000000000000000000000000000000000000000000000000000046)]
// #[raw(0x2e1a7d4d0000000000000000000000000000000000000000000000000000000000000014, label = "withdraw(20) — burns WETH and refunds ETH to caller") -> success(0x)]
// #[raw(0x70a082310000000000000000000000001212121212121212121212121212120000000012, label = "balanceOf(caller) -> 50") -> success(0x0000000000000000000000000000000000000000000000000000000000000032)]
// #[raw(0x18160ddd, label = "totalSupply() -> 80 (after 20 wei withdrawn)") -> success(0x0000000000000000000000000000000000000000000000000000000000000050)]
// #[raw(0x095ea7b300000000000000000000000070997970c51812dc3a010c7d01b50e0d17dc79c80000000000000000000000000000000000000000000000000000000000000019, label = "approve($ANVIL1, 25) -> true") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0xdd62ed3e000000000000000000000000121212121212121212121212121212000000001200000000000000000000000070997970c51812dc3a010c7d01b50e0d17dc79c8, label = "allowance(caller, $ANVIL1) -> 25") -> success(0x0000000000000000000000000000000000000000000000000000000000000019)]
// #[raw(0x2e1a7d4d00000000000000000000000000000000000000000000000000000000000003e8, label = "withdraw(1000) — InsufficientBalance() revert") -> revert(0xf4d678b8)]
// #[raw(0xdeadbeef, value = 7, label = "fallback with unknown selector + value=7 auto-wraps ETH") -> success(0x)]
// #[raw(0x70a082310000000000000000000000001212121212121212121212121212120000000012, label = "balanceOf(caller) -> 57 (50 + 7 via fallback)") -> success(0x0000000000000000000000000000000000000000000000000000000000000039)]
// #[raw(0x18160ddd, label = "totalSupply() -> 87 (80 + 7)") -> success(0x0000000000000000000000000000000000000000000000000000000000000057)]
contract WETH9 {
    balances: mapping(address => uint256);
    allowance: mapping(address => mapping(address => uint256));

    constructor() {}

    // --- ETH <-> WETH ---

    function deposit() public payable {
        let sender = caller();
        balances[sender] = balances[sender] + callvalue();
    }

    function withdraw(wad: uint256) public {
        let sender = caller();
        require(balances[sender] >= wad, Error(0xf4d678b8)); // InsufficientBalance()
        balances[sender] = balances[sender] - wad;
        sendValue(sender, wad);
    }

    // totalSupply == ETH held by this contract (matches canonical WETH9).
    function totalSupply() public returns (uint256) {
        return uint256(selfbalance());
    }

    // --- ERC20 surface ---

    function balanceOf(account: address) public returns (uint256) {
        return balances[account];
    }

    function allowance(owner_: address, spender: address) public returns (uint256) {
        return allowance[owner_][spender];
    }

    function approve(usr: address, wad: uint256) public returns (bool) {
        let sender = caller();
        allowance[sender][usr] = wad;
        return true;
    }

    function transfer(dst: address, wad: uint256) public returns (bool) {
        return transferFrom(caller(), dst, wad);
    }

    function transferFrom(src: address, dst: address, wad: uint256) public returns (bool) {
        let sender = caller();
        require(balances[src] >= wad, Error(0xf4d678b8)); // InsufficientBalance()

        if (src != sender && allowance[src][sender] != (maxVal())) {
            require(allowance[src][sender] >= wad, Error(0x13be252b)); // InsufficientAllowance()
            allowance[src][sender] -= wad;
        }
        balances[src] = balances[src] - wad;
        balances[dst] = balances[dst] + wad;
        return true;
    }

    // Plain ETH transfers (no calldata, just value) auto-wrap into WETH.
    fallback() payable {
        let sender = caller();
        balances[sender] = balances[sender] + callvalue();
    }
}
