function hash_word(value: word) -> word {
  let result: word;
  assembly {
    mstore(0, value)
    result := keccak256(0, 32)
  }
  return result;
}
