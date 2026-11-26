pragma solcore;

function main() returns (word) {
    let sender = msg.sender;
    revert("failed");
    return keccak256(sender);
}
