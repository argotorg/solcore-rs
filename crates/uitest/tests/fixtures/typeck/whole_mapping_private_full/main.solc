data address = address(word);
data uint256 = uint256(word);
data mapping(index, member) = mapping(word);
data storage(t) = storage(word);

contract C {
  balances : mapping(address, uint256);

  function leak() -> mapping(address, uint256) {
    return balances;
  }

  function main() -> word {
    return 0;
  }
}
