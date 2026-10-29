pragma solcore;

trait Stamp<self> {
    function stamp(value: self, comptime tag: word) returns (word);
}

impl Stamp<word> {
    function stamp(value: word, comptime tag: word) returns (word) {
        return tag;
    }
}

function parameterLiteral(value: word) returns (word) {
    return value.stamp(7);
}

function localLiteral(value: word) returns (word) {
    let local: word = value;
    return local.stamp(7);
}

function parameterRuntime(value: word, tag: word) returns (word) {
    return value.stamp(tag);
}

function localRuntime(value: word, tag: word) returns (word) {
    let local: word = value;
    return local.stamp(tag);
}
