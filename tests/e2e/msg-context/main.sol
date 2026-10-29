pragma solcore;

import * from std;
import * from std.dispatch;

// #[scenario("msg_context")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x67e404ce, label = "sender()(address) - returns the caller") -> success(0x0000000000000000000000001212121212121212121212121212120000000012)]
// #[raw(0xd5f39488, label = "deployer()(address) - msg_sender() in the constructor is the deployer") -> success(0x0000000000000000000000001212121212121212121212121212120000000012)]
// #[raw(0x3fa4f245, value = 42, label = "value()(uint256) - returns the value sent") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0x3fa4f245, label = "value()(uint256) - zero when no value is sent") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
contract MsgContext {
    deployer: address;

    constructor() {
        deployer = msgSender();
    }

    function sender() public returns (address) {
        return msgSender();
    }

    function deployer() public returns (address) {
        return deployer;
    }

    function value() public payable returns (uint256) {
        return msgValue();
    }
}
