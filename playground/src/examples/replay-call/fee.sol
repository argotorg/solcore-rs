pragma solcore;

// The fee is 13. Try changing it to 12 after replaying checkout(8).
function withFee(amount: word) returns (word) {
    let total: word;
    assembly { total := add(amount, 13) }
    return total;
}
export { withFee };
