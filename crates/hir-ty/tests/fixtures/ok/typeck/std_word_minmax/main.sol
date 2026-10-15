import { Typedef, maxWord, minWord, uint256 } from std;

function minUint(a: uint256, b: uint256) returns (uint256) {
    return uint256(minWord(Typedef.rep(a), Typedef.rep(b)));
}

function maxUint(a: uint256, b: uint256) returns (uint256) {
    return uint256(maxWord(Typedef.rep(a), Typedef.rep(b)));
}
