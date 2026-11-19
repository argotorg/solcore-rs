pragma solcore;

export { Imported(*), Reexported(*) };

enum Imported {
    A,
    B
}
enum Reexported {
    A,
    B(word)
}
