pragma solcore;

// `A` arrives through a re-export only; its impl in types.sol is still visible.
import { Fee } from fees;
import { A } from facade;

function main() returns (word) {
    return Fee.fee(A);
}
