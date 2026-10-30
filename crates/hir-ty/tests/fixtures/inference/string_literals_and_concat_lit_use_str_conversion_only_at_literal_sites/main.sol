pragma solcore;

import std;
import { memory, string, strlenLit } from std;

enum Tag {
    Tag(word)
}
impl Str<Tag> {
    function fromString(comptime value: string) returns (Tag) {
        return Tag(strlenLit(value));
    }
}

function literal() returns (memory<string>) {
    return "hello";
}
function concatLit(lhs: word, rhs: word) returns (word) {
    return lhs;
}
function concatenated() returns (memory<string>) {
    return std.concatLit("he", "llo");
}
function explicit(value: string) returns (memory<string>) {
    return Str.fromString(value);
}
function tagged() returns (Tag) {
    return "abcd";
}
function taggedFromLet() returns (Tag) {
    let value = "abcd";
    return Str.fromString(value);
}
function inferredLiteral() returns () {
    let value = "x";
    return ();
}
function inferredConcat() returns () {
    let value = std.concatLit("a", "b");
    return ();
}
function consumePair(value: (string, word)) returns (word) {
    return 0;
}
function inferredTuple() returns (word) {
    let value = ("x", 0);
    return consumePair(value);
}
function inferredComptimeParam() returns () {
    let sink = lam (comptime value: string) -> () {
        return ();
    };
    sink("x");
    return ();
}
function invalidConcat() returns (memory<string>) {
    return concatLit(1, 2);
}
function makeWord() returns (word) {
    return 0;
}
function invalidSource() returns (memory<string>) {
    let value;
    let result: memory<string> = Str.fromString(value);
    value = makeWord();
    return result;
}
function runtime(value: string) returns (memory<string>) {
    return value;
}
