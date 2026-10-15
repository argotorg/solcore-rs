enum Choice {
    Same(word),
    Same(bool)
}

function ambiguous() returns (Choice) {
    return .Same(1);
}
