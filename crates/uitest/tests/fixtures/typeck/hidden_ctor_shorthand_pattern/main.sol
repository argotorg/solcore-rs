pragma solcore;

import { Token } from lib;

function main(t: Token) returns (word) {
    match (t) {
        case .Ok(x) {
            return x;
        }
        case .Err(x) {
            return x;
        }
    }
}
