pragma solcore;

export { Token(Ok), Reply(Done) };

enum Token {
    Ok(word),
    Err(word)
}

enum Reply {
    Done(word),
    Err(word)
}
