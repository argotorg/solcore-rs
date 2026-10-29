pragma solcore;

import { memory, string } from std;

function alpha() returns (memory<string>) {
    return "alpha";
}
function beta() returns (memory<string>) {
    return "beta";
}
function main() returns (memory<string>) {
    alpha();
    beta();
    return "alpha";
}
