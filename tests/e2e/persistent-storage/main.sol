import * from std;
import * from std.dispatch;

contract PersistentStorage {
    stored: uint256;

    constructor() {
        stored = uint256(7);
    }

    // #[() -> 7]
    function initialValue() public returns (uint256) {
        return stored;
    }

    // #[send(41)]
    function setStored(value: uint256) public {
        stored = value;
    }

    // #[() -> 41]
    function valueAfterSend() public returns (uint256) {
        return stored;
    }
}
