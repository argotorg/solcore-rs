pragma solcore;

export { Secret, makeSecret };

struct Secret {
    value: word;
}

function makeSecret() returns (Secret) {
    return Secret(42);
}
