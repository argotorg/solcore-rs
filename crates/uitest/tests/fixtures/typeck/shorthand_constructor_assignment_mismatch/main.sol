enum Option {
    None,
    Some(word)
}

function bad() returns (word) {
    let x: Option;
    x = .Some(true);
    return 0;
}
