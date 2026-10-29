export { string, address(*), bytes, bytes32(*), memory(*) };
enum string {}
enum address {
    address(word)
}
enum bytes {}
enum bytes32 {
    bytes32(word)
}
enum memory<t> {
    memory(word)
}
