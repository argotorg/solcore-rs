pragma solcore;

// Even an unused declaration in an imported module must be checked.
function unused() returns (word) {
    return missing;
}
