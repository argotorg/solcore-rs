import * from std;
import * from std.dispatch;

// caller() is not in the std library yet,
// so every contract must define its own

function caller() returns (address) {
    let res: word;
    assembly {
        res := caller()
    }
    return address(res);
}

// #[scenario("ownable")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x893d20e8, label = "getOwner()(address) - should return deployer address") -> success(0x0000000000000000000000001212121212121212121212121212120000000012)]
// #[raw(0xa6f9dae1000000000000000000000000000000000000000000000000000000000000dead, label = "changeOwner(address) to 0xdead - should succeed from owner") -> success(0x)]
// #[raw(0x893d20e8, label = "getOwner()(address) - should now return 0xdead") -> success(0x000000000000000000000000000000000000000000000000000000000000dead)]
// #[raw(0xa6f9dae1000000000000000000000000000000000000000000000000000000000000beef, label = "changeOwner(address) - should fail, caller is no longer owner") -> revert(0x12b0c500)]
contract Ownable {
    owner: address;

    constructor() {
        owner = caller();
    }

    // named getOwner() instead of owner() to avoid collision with the field name
    function getOwner() public returns (address) {
        return owner;
    }

    function changeOwner(newOwner: address) public {
        require(caller() == owner, Error(0x12b0c500)); // OwnableUnauthorizedAccount()
        owner = newOwner;
    }
}
