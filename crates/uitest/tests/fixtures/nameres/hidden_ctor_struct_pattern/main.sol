pragma solcore;

import { Box } from lib;

function main(b: Box) returns (word) {
    match (b) {
        case Box(x) {
            return x;
        }
    }
}
