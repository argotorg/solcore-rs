pragma solcore;

export { memory(*), string, concatLit, strlenLit };
enum memory<a> {
    memory(word)
}
enum string {}
function concatLit(comptime lhs: string, comptime rhs: string) returns (string) {
    return lhs;
}
function strlenLit(comptime value: string) returns (word) {
    return 0;
}
