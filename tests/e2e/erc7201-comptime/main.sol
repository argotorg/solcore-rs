pragma solcore;

import * from std;
import * from std.dispatch;

contract Erc7201Comptime {
    // keccak256(bytes32(0))
    // #[() -> 0x290decd9548b62a8d60345a988386fc84ba6bc95484008f6362f93160ef3e563]
    function keccakWord() public returns (bytes32) {
        return bytes32(keccakWordLit(0));
    }

    // ERC-7201 namespace constants are folded at compile time.
    // #[() -> 0x183a6125c38840424c4a85fa12bab2ab606c4b6d0e7cc73c0c06ba5300eab500]
    function example() public returns (bytes32) {
        return erc7201("example.main");
    }

    // #[() -> 0x9016d09d72d40fdae2fd8ceac6b6234c7706214fd39c1cd1e609a0528c199300]
    function ownable() public returns (bytes32) {
        return erc7201("openzeppelin.storage.Ownable");
    }

    // #[() -> 0x4318a0031e4d2f411be9017543511db04d79cf580aaff6bae7539a4a49eacc00]
    function empty() public returns (bytes32) {
        return erc7201("");
    }
}
