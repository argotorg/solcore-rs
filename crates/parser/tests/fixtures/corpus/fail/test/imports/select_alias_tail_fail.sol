pragma solcore;

import { keep } from selectlib;

function main(x: word) returns (word) {
    return keep_(x);
}
