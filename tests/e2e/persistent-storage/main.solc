import std.{*};
import std.dispatch.{*};

contract PersistentStorage {
  stored: uint256;

  constructor() {
    stored = uint256(7);
  }

  // #[() -> 7]
  public function initialValue() -> uint256 {
    return stored;
  }

  // #[send(41)]
  public function setStored(value: uint256) {
    stored = value;
  }

  // #[() -> 41]
  public function valueAfterSend() -> uint256 {
    return stored;
  }
}
