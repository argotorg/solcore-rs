pragma solcore;

import * from std;
import * from std.dispatch;

// #[scenario("constructorBounds")]
// Constructor rejects a dynamic byte payload shorter than its declared length
// #[deploy(0x00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000004010203, value = 0) -> revert(0x08638556)]
contract ConstructorBounds {
    payload: bytes;
    constructor(value: memory<bytes>) {
        payload = value;
    }
    function read() public returns (memory<bytes>) {
        return payload;
    }
}
