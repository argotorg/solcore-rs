import * from std;
import * from std.dispatch;
import { mstore } from std.opcodes;

// Build a memory(bytes) holding the three-byte string "abc".
function abcBytes() returns (memory<bytes>) {
    let p = allocate_memory(64);
    mstore(p, 3);
    mstore(p + 32, 0x6162630000000000000000000000000000000000000000000000000000000000);
    return memory(p);
}

// #[scenario("hashes")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x775dec49, label = "keccak()(bytes32)") -> success(0x4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45)]
// #[raw(0x65707d6f, label = "sha()(bytes32)") -> success(0xba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad)]
// #[raw(0xa1e754ea, label = "ripemd()(bytes32)") -> success(0x0000000000000000000000008eb208f7e05d987a9b044a8e98c6b087f15a0bfc)]
// #[raw(0x6c632152, label = "keccakWord()(bytes32) -- comptime keccakWordLit(0) == keccak256(bytes32(0))") -> success(0x290decd9548b62a8d60345a988386fc84ba6bc95484008f6362f93160ef3e563)]
// #[raw(0x36663f67, label = "erc7201Example()(bytes32) -- comptime erc7201(\"example.main\")") -> success(0x183a6125c38840424c4a85fa12bab2ab606c4b6d0e7cc73c0c06ba5300eab500)]
// #[raw(0xc4bc0281, label = "erc7201Ownable()(bytes32) -- comptime erc7201(\"openzeppelin.storage.Ownable\")") -> success(0x9016d09d72d40fdae2fd8ceac6b6234c7706214fd39c1cd1e609a0528c199300)]
// #[raw(0x8fdc47eb, label = "erc7201Empty()(bytes32) -- comptime erc7201(\"\")") -> success(0x4318a0031e4d2f411be9017543511db04d79cf580aaff6bae7539a4a49eacc00)]
contract C {
    constructor() {}

    function keccak() public returns (bytes32) {
        return keccak256_(abcBytes());
    }

    function sha() public returns (bytes32) {
        return sha256(abcBytes());
    }

    function ripemd() public returns (bytes32) {
        return ripemd160(abcBytes());
    }

    // keccakWordLit folds keccak256 of a word's 32-byte big-endian form at
    // compile time; keccakWordLit(0) == keccak256(bytes32(0)).
    function keccakWord() public returns (bytes32) {
        return bytes32(keccakWordLit(0));
    }

    // ERC-7201 namespaced storage slots, folded to constants at compile time
    // from the string-literal namespace (no runtime keccak of the id).
    function erc7201Example() public returns (bytes32) {
        return erc7201("example.main");
    }

    function erc7201Ownable() public returns (bytes32) {
        return erc7201("openzeppelin.storage.Ownable");
    }

    function erc7201Empty() public returns (bytes32) {
        return erc7201("");
    }
}
