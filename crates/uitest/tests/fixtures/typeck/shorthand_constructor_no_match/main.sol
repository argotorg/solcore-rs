pragma solcore;

enum Other {
    Other
}

function noMatch() returns (Other) {
    return .Some(1);
}
