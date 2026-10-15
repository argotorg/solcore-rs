import * from std;
import * from std.dispatch;

function caller() returns (address) {
    let res: word;
    assembly {
        res := caller()
    }
    return address(res);
}

// #[scenario("miniERC20")]
// 'constructor(string,string,uint)' Argot ARG 1000
// #[deploy(0x000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000003e800000000000000000000000000000000000000000000000000000000000000054172676f7400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000034152470000000000000000000000000000000000000000000000000000000000) -> success]
// #[raw(0xa9059cbb00000000000000000000000070997970c51812dc3a010c7d01b50e0d17dc79c800000000000000000000000000000000000000000000000000000000000003be, label = "transfer(address,uint)(bool)  $ANVIL1  958") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x4c738909, label = "getMyBalance()") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0xa9059cbb00000000000000000000000070997970c51812dc3a010c7d01b50e0d17dc79c80000000000000000000000000000000000000000000000000000000000000064, label = "transfer(address,uint)(bool)  $ANVIL1  100 -- over balance, reverts with the string literal passed to require") -> revert(0x7472616e7366657246726f6d3a20696e73756666696369656e742062616c616e6365)]
contract MiniERC20 {
    name: string;
    symbol: string;
    owner: address;
    decimals: uint256; // should be uint8 when we get to it
    totalSupply: uint256;
    balances: mapping(address => uint256);
    allowance: mapping(address => mapping(address => uint256));

    constructor(name_: memory<string>, symbol_: memory<string>, totalSupply_: uint256) {
        name = name_;
        symbol = symbol_;
        owner = caller();
        decimals = 18;
        mint(totalSupply_);
    }

    function name() public returns (memory<string>) {
        return name;
    }

    function symbol() public returns (memory<string>) {
        return symbol;
    }

    function decimals() public returns (uint256) {
        return decimals;
    }

    function allowance(owner_: address, spender: address) public returns (uint256) {
        return allowance[owner_][spender]; // don't use "owner" here
    }

    function balanceOf(account: address) public returns (uint256) {
        return balances[account];
    }

    function totalSupply() public returns (uint256) {
        return totalSupply;
    }

    // Note that this is not access guarded — the minting always goes to the owner
    function mint(amount: uint256) public {
        balances[owner] = Num.add(balances[owner], amount);
        totalSupply = Num.add(totalSupply, amount);
    }

    function transfer(dst: address, amt: uint256) public returns (bool) {
        return transferFrom(caller(), dst, amt);
    }

    function transferFrom(src: address, dst: address, amt: uint256) public returns (bool) {
        let msg_sender = caller();
        require(balances[src] >= amt, "transferFrom: insufficient balance");

        if (src != msg_sender && allowance[src][msg_sender] != (Num.maxVal())) {
            require(allowance[src][msg_sender] >= amt, "transferFrom: insufficient allowance");
            allowance[src][msg_sender] -= amt;
        }
        balances[src] = balances[src] - amt;
        balances[dst] = balances[dst] + amt;
        // emit Transfer(src, dst, amt);
        return true;
    }

    function approve(usr: address, amt: uint256) public returns (bool) {
        let msg_sender = caller();
        allowance[msg_sender][usr] = amt;
        // emit Approval(msg.sender, usr, amt);
        return true;
    }

    // testing
    function getMyBalance() public returns (uint256) {
        return balances[caller()];
    }

    function test() public returns (uint256) {
        approve(address(0), 10);
        transferFrom(caller(), address(0), 958);
        return getMyBalance();
    }

}
