import * from std;

type U = word;

contract Signatures {
    function spell(a: word, b: (word, bool), c: memory<string>, d: memory<bytes>, e: bytes32, f: address, g: U) public returns (word) {
        return a;
    }
}
