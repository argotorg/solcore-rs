import * from std;
import * from std.dispatch;

contract StdWordCorrectness {
    // #[(7, 42) -> 7]
    // #[(42, 7) -> 7]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0) -> 0]
    function minOf(a: uint256, b: uint256) public returns (uint256) {
        return uint256(minWord(Typedef.rep(a), Typedef.rep(b)));
    }

    // #[(7, 42) -> 42]
    // #[(42, 7) -> 42]
    // #[(0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, 0) -> 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff]
    function maxOf(a: uint256, b: uint256) public returns (uint256) {
        return uint256(maxWord(Typedef.rep(a), Typedef.rep(b)));
    }

    // An invalid recovery id makes the precompile succeed with no returndata.
    // Dirty scratch memory must not be mistaken for a recovered address.
    // #[() -> revert(0x4fbfae63)]
    function recoverInvalidAfterDirtyScratch() public returns (address) {
        assembly {
            mstore(0, 0x1234)
        }
        let h: bytes32 = bytes32(0xaabbccddeeff00112233445566778899aabbccddeeff00112233445566778899);
        let v: uint256 = uint256(1);
        let r: bytes32 = bytes32(0xb3ba6dd3757d18f28736e84b1296af85362b7bdf4548710733c6325abf95311d);
        let s: bytes32 = bytes32(0x3523e7d34da277c59af090e44cebddb10b73be11780f028d02cf5ae5f24109fc);
        return ecrecover(h, v, r, s);
    }
}
