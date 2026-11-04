pragma solcore;

// Core Solidity has no events yet, so this module writes the Transfer log with the log3 opcode.
import * from std;
import { log3, mstore } from std.opcodes;

export { emitTransfer };

// Emits Transfer(from, to, amount) with the given topic.
function emitTransfer(topic: bytes32, from: address, to: address, amount: uint256) {
    mstore(0, Typedef.rep(amount));
    log3(0, 32, Typedef.rep(topic), Typedef.rep(from), Typedef.rep(to));
}
