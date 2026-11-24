pragma solcore;

export { Token(Ok), Box, mkErr };

enum Token {
    Ok(word),
    Err(word)
}

struct Box {
    value: word;
}

function mkErr(x: word) returns (Token) {
    return Token.Err(x);
}
