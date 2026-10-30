pragma solcore;

trait Echo<self> {
    function echo(value: self) returns (self);
}

impl Echo<word> {
    function echo(value: word) returns (word) {
        return value;
    }
}

function parameterReceiver(value: word) returns (word) {
    return value.echo();
}

function localReceiver(value: word) returns (word) {
    let local: word = value;
    return local.echo();
}
