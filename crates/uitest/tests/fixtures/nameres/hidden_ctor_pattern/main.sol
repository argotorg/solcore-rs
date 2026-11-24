pragma solcore;

import { Token } from lib;

function main(t: Token) returns (word) {
    match (t) {
        case Token.Ok(x) {
            return x;
        }
        case Token.Err(x) {
            return x;
        }
    }
}
